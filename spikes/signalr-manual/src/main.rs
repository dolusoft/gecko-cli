//! Runs one trial scenario against a hub and prints what happened.
//! Usage: signalr-manual-trial <hub-url> <token> status|idle <seconds>
//!        [--negotiate] [--keepalive-ms <n>] [--server-timeout-ms <n>]
use std::time::{Duration, Instant};

use serde_json::value::RawValue;
use signalr_manual_trial::{hub_result, Connection, Options};

fn flag_ms(args: &[String], name: &str) -> Option<Duration> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1)?.parse().ok().map(Duration::from_millis)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: signalr-manual-trial <hub-url> <token> status|idle <seconds> [--negotiate] [--keepalive-ms <n>] [--server-timeout-ms <n>]");
        std::process::exit(2);
    }
    let mut options = Options::new(&args[1], &args[2]);
    options.skip_negotiation = !args.iter().any(|a| a == "--negotiate");
    if let Some(value) = flag_ms(&args, "--keepalive-ms") {
        options.keep_alive = value;
    }
    if let Some(value) = flag_ms(&args, "--server-timeout-ms") {
        options.server_timeout = value;
    }

    let started = Instant::now();
    let mut connection = match Connection::connect(&options).await {
        Ok(connection) => connection,
        Err(reason) => {
            println!("connect: {reason:?} after {:?}", started.elapsed());
            std::process::exit(1);
        }
    };
    println!("connected after {:?}", started.elapsed());

    if args[3] == "idle" {
        let seconds: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(130);
        println!("idling {seconds} s");
        tokio::time::sleep(Duration::from_secs(seconds)).await;
    }

    let call = Instant::now();
    match connection.invoke("Status", &[]).await {
        Ok(result) => println!(
            "Status: {:?} in {:?}",
            hub_result(&result).map(|v| v.get().to_owned()),
            call.elapsed()
        ),
        Err(error) => println!("Status failed: {error:?}"),
    }
    let envelope = RawValue::from_string("{\"kind\":\"echo\",\"text\":\"hi\"}".to_owned()).unwrap();
    let call = Instant::now();
    let sent = connection.invoke("Send", &[&envelope]).await;
    let event = connection.next_event().await;
    println!("Send: {sent:?} event: {event:?} in {:?}", call.elapsed());
    println!(
        "closed: {:?}, total {:?}",
        connection.close().await,
        started.elapsed()
    );
}
