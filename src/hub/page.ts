// The development hub's stand-in for the login approval page of the web app
// (§8): it reads the code from the address, removes it, and approves it for
// the fake user. The real page lives in frontendx.

export function loginPage(userId: string): string {
  return `<!doctype html>
<html lang="tr">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>gecko dev hub: CLI girişi</title>
<body style="font-family: system-ui, sans-serif; max-width: 32rem; margin: 3rem auto; padding: 0 1rem">
<h1>CLI girişi (geliştirme hub'ı)</h1>
<p>Sahte kullanıcı: <code>${userId}</code></p>
<form id="f">
  <label>Kod <input id="code" autocomplete="off" required></label>
  <button name="approve" value="true">Onayla</button>
  <button name="approve" value="false">Reddet</button>
</form>
<p id="out" role="status"></p>
<script>
  const code = location.hash.slice(1)
  if (code) {
    document.getElementById('code').value = decodeURIComponent(code)
    history.replaceState(null, '', location.pathname)
  }
  document.getElementById('f').addEventListener('submit', async event => {
    event.preventDefault()
    const approve = event.submitter && event.submitter.value === 'true'
    const response = await fetch('/oauth/device/verify', {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: 'Bearer dev:${userId}' },
      body: JSON.stringify({ user_code: document.getElementById('code').value, approve })
    })
    document.getElementById('out').textContent = response.ok
      ? (approve ? 'Onaylandı; terminale dönebilirsiniz.' : 'Reddedildi.')
      : 'Kod geçersiz ya da süresi dolmuş.'
  })
</script>
</body>
</html>
`
}
