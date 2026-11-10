# Security

Please report a vulnerability privately through GitHub: the **Security** tab of this
repository, then **Report a vulnerability**. Do not open a public issue for it.

`gecko` keeps one secret on disk, the CLI token, readable by the signed-in user only. It
sends the token in the `Authorization` header only, never in an address.
