//! TCP connect that fails fast on a closed local port.
//!
//! Windows answers a refused connection by sending the SYN again, twice, so a call to a
//! hub that is not running took two seconds to say so. For a loopback address the
//! socket is told not to retry the SYN before connecting, as libuv (Node) does; a hub
//! on this machine either listens or it does not. Other systems answer at once.

use std::io;
use std::net::SocketAddr;

use tokio::net::{TcpSocket, TcpStream};

/// Connects to the first address of `host:port` that accepts.
pub async fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
    let mut last = None;
    for address in tokio::net::lookup_host((host, port)).await? {
        match connect_to(address).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, format!("{host}: no address found"))
    }))
}

async fn connect_to(address: SocketAddr) -> io::Result<TcpStream> {
    let socket = if address.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    if address.ip().is_loopback() {
        no_syn_retries(&socket);
    }
    socket.connect(address).await
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn no_syn_retries(socket: &TcpSocket) {
    use std::os::windows::io::AsRawSocket;

    use windows_sys::Win32::Networking::WinSock::{
        SIO_TCP_INITIAL_RTO, SOCKET, TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS,
        TCP_INITIAL_RTO_PARAMETERS, WSAIoctl,
    };

    // libuv's values: no retransmission timer, and the "no SYN retransmissions"
    // marker, which the structure's byte field holds as 0xFE.
    let parameters = TCP_INITIAL_RTO_PARAMETERS {
        Rtt: u16::MAX,
        MaxSynRetransmissions: (TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS & 0xff) as u8,
    };
    let mut returned = 0u32;
    let raw = socket.as_raw_socket() as SOCKET;
    // SAFETY: `raw` is the open socket `socket` owns for the whole call; the input
    // points at a live, correctly sized `TCP_INITIAL_RTO_PARAMETERS`; there is no
    // output buffer, no overlapped structure and no completion routine. A failure
    // only leaves the system default (the slow refusal), so its result is ignored.
    unsafe {
        WSAIoctl(
            raw,
            SIO_TCP_INITIAL_RTO,
            std::ptr::from_ref(&parameters).cast(),
            u32::try_from(size_of::<TCP_INITIAL_RTO_PARAMETERS>()).unwrap_or(0),
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
            None,
        );
    }
}

#[cfg(not(windows))]
fn no_syn_retries(_socket: &TcpSocket) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_closed_local_port_is_refused_at_once() {
        // A port that was just open and is closed now.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let started = std::time::Instant::now();
        let error = connect("127.0.0.1", port).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn an_open_local_port_connects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (connected, accepted) = tokio::join!(connect("localhost", port), listener.accept());
        assert!(connected.is_ok() && accepted.is_ok());
    }
}
