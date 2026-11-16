# Public, disposable test identities only. Never use these keys outside tests.
$ErrorActionPreference = 'Stop'
$now = Get-Date
foreach ($name in @('first', 'rotated', 'expired')) {
    $rsa = [System.Security.Cryptography.RSA]::Create(2048)
    $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
        'CN=localhost', $rsa, [System.Security.Cryptography.HashAlgorithmName]::SHA256,
        [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
    $san = [System.Security.Cryptography.X509Certificates.SubjectAlternativeNameBuilder]::new()
    $san.AddDnsName('localhost')
    $san.AddIpAddress([System.Net.IPAddress]::Loopback)
    $request.CertificateExtensions.Add($san.Build())
    $request.CertificateExtensions.Add([System.Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($false, $false, 0, $true))
    $usage = [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature -bor [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::KeyEncipherment
    $request.CertificateExtensions.Add([System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new($usage, $true))
    $purposes = [System.Security.Cryptography.OidCollection]::new()
    $purposes.Add([System.Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.1')) | Out-Null
    $request.CertificateExtensions.Add([System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($purposes, $false))
    $start = $now.AddDays(-1)
    $end = $now.AddDays(364)
    if ($name -eq 'expired') {
        $end = $now.AddYears(-1)
        $start = $end.AddDays(-365)
    }
    $cert = $request.CreateSelfSigned($start, $end)
    [System.IO.File]::WriteAllBytes((Join-Path $PSScriptRoot "$name.der"), $cert.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Cert))
    [System.IO.File]::WriteAllBytes((Join-Path $PSScriptRoot "$name-key.der"), $rsa.ExportPkcs8PrivateKey())
    $cert.Dispose()
    $rsa.Dispose()
}
