//! The development hub's stand-in for the web app's login approval page (§8): it reads
//! the code from the address, removes it, and approves it for the fake user. The real
//! page lives in the Gecko web app.

/// The page for `GET /cli-login`. `user_id` is a configured fake user's id.
pub fn login_page(user_id: &str) -> String {
    let user = html_escape(user_id);
    let token = js_string(&format!("Bearer dev:{user_id}"));
    format!(
        r#"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>gecko dev hub: CLI sign-in</title>
<body style="font-family: system-ui, sans-serif; max-width: 32rem; margin: 3rem auto; padding: 0 1rem">
<h1>CLI sign-in (development hub)</h1>
<p>Fake user: <code>{user}</code></p>
<form id="f">
  <label>Code <input id="code" autocomplete="off" required></label>
  <button name="approve" value="true">Approve</button>
  <button name="approve" value="false">Deny</button>
</form>
<p id="out" role="status"></p>
<script>
  const code = location.hash.slice(1)
  if (code) {{
    document.getElementById('code').value = decodeURIComponent(code)
    history.replaceState(null, '', location.pathname)
  }}
  document.getElementById('f').addEventListener('submit', async event => {{
    event.preventDefault()
    const approve = event.submitter && event.submitter.value === 'true'
    const response = await fetch('/oauth/device/verify', {{
      method: 'POST',
      headers: {{ 'content-type': 'application/json', authorization: {token} }},
      body: JSON.stringify({{ user_code: document.getElementById('code').value, approve }})
    }})
    document.getElementById('out').textContent = response.ok
      ? (approve ? 'Approved; you can go back to the terminal.' : 'Denied.')
      : 'The code is wrong or has expired.'
  }})
</script>
</body>
</html>
"#
    )
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A JavaScript string literal that is safe inside a `<script>` element.
fn js_string(text: &str) -> String {
    serde_json::to_string(text)
        .unwrap_or_else(|_| "\"\"".into())
        .replace('<', "\\u003c")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_cannot_break_out_of_the_page() {
        let page = login_page("</script><b>");
        assert!(!page.contains("</script><b>"));
        assert!(page.contains("&lt;/script&gt;&lt;b&gt;"));
        let lt = ["\\", "u003c"].concat();
        assert!(page.contains(&format!("\"Bearer dev:{lt}/script>{lt}b>\"")));
    }
}
