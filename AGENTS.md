# Ajan rehberi

Bir şeyi değiştirmeden önce oku. Kısa tutuldu; ayrıntı bağlantılı dosyalarda.

## Ne bu repo

`gecko` CLI'ı ve masaüstü CLI spec'inin §10.3 sözleşmesini uygulayan geliştirme hub'ı. Spec kanoniktir; buradaki davranış ondan sapıyorsa sapma PR'da yazılır.

Repo public (MIT). Spec ve geçiş planı Dolusoft'un iç belgeleridir, repoda yoktur: kod yorumlarındaki `§` numaraları spec'in, `spec §7.x` geçiş planının, `#1974` Gecko backend'inin hub kaydının, `K1`, `F30` gibi kodlar geçiş planındaki kararların kimliğidir. Repoya iç belge, özel repo bağlantısı ya da müşteri bilgisi girmez.

## Düzen

Ürün Rust ikilisidir (`crates/`). Bugün `login`, `logout`, `attach`, `detach`, `status`, `help` ve sekmeye satır göndermeyi yapar (command-console 1.7.2; yalnız geliştirme hub'ına karşı).

- `crates/gecko/`: ikili. `args.rs` seçenekler (`args_os`, geçersiz Unicode'da kod 2), `main.rs` yardım, sürüm, `dev-hub` anahtarı ve komut dağıtımı; `own.rs` `login`/`logout`; `binding.rs` `attach`/`detach`/`status`; `help.rs` `gecko help` (`help-request`, sekmenin metni ya da JSON sonucu); `line.rs` sekmeye giden satır (ön süzgeç: hassas ad CLI'dan çıkmaz, listede olmayan addan yalnız ad gider; yetenek listesi, gönderim, bekleyiş); `run.rs` gönderilmiş isteğin saf durum makinesi (cevaplar, kopma, teslim süresi, Ctrl+C girer, sonuç çıkar; `tests/sequence.rs` 1.7.0 `sequence/` örneklerini buna koşar); `session.rs` hub oturumu, kapanış nedeninin çıkış koduna eşlenmesi, eşleştirme; `auth.rs` cihaz girişi, yenileme, iptal; `config.rs` dosyalar ve token izinleri; `output.rs` sonuç, çıkış kodları, kabuk tırnaklaması, bekleme göstergesi; `format.rs` `data`'nın düz görünümü (tablo, ızgara, `key: value`); `interrupt.rs` Ctrl+C; `browser.rs`. `tests/` ikiliyi süreç olarak başlatır; karşısındaki hub ve sekme testin içinde betiklenmiştir, gerçek geliştirme hub'ı değildir.
- `crates/gecko-wire/`: sekmeyle sözleşme (command-console 1.7.2) ve `--json` için ASCII JSON yazıcısı. `reply.rs` sekmenin cevaplarını alan alan okur (bilinmeyen `kind` atlanır, bilinmeyen `status`/`class` tanımlı varsayılana düşer); `request.rs` komut adı kuralı, gönderim öncesi sınırlar ve zarflar; `capabilities.rs` parçalı yetenek listesi, gereken `features`, `minCli`. `tests/fixtures.rs` `fixtures/command-console-1.7.2/` örneklerini koşar; o klasör yayın ekinden olduğu gibi kopyalanır, elle düzenlenmez ([fixtures/README.md](fixtures/README.md)).
- `crates/gecko-hub-client/`: `Transport` trait'i (`status`, `pair`, `send`, `detach`, `next_event`), anlamsal `Closed` nedenleri, `HubError`, `HubEvent`. `hub.rs` hub'ın kendi cevapları (`HubStatus`, `Binding`, `PairCode`; bilinmeyen durum `Unknown`). `dev.rs` geliştirme hub'ının JSON çerçeveleri (yalnız localhost); `select.rs` taşıma seçimi (`GECKO_TRANSPORT`); `signalr/` elle yazılmış dar SignalR istemcisi (hazır crate token'ı adreste taşıdığı için); `tcp.rs` hızlı reddedilen loopback bağlantısı (Windows, tek `unsafe`); `net.rs` TCP/TLS ve vekil tüneli; `proxy.rs` `*_proxy`/`no_proxy` kuralı.
- `crates/gecko-dev-hub/`: `gecko dev-hub` (K1). `hub.rs` kayıtlar ve kurallar (cihaz girişi, token, bağlama, damga, hız sınırı, heartbeat); `server.rs` HTTP/1.1 ve `/hub` WebSocket yükseltmesi, bağlantı başına tek istek; `page.rs` `/cli-login` sayfası; `random.rs` kod ve sırlar. `dev-hub` cargo özelliğinin arkasında: varsayılan derleme özelliği açar, sürüm derlemesi `--no-default-features` ile yapılır ve "this build has no dev hub" der. `tests/hub.rs` hub'ı süreç içinde, ham HTTP ve WebSocket ile dener; gerçek CLI ile gerçek hub'ı birlikte koşan test yok.
- `tests/signalr-hub/`: SignalR istemci testleri için en küçük ASP.NET Core hub'ı (CI `signalr` işi).
- Yayın: `.github/workflows/release.yml`. Akış: `Cargo.toml`'daki `version` ve `CHANGELOG.md`'deki `## <sürüm>` bölümü aynı PR'da yazılır. Merge'den sonra iş akışı elle çalıştırılır; bu deneme yalnız derler. Sonra `v<sürüm>` tag'i atılır. Tag altı hedefi `--no-default-features` ile derler, Release'i `SHA256SUMS` ve CHANGELOG bölümüyle açar, sonra `install.sh` / `install.ps1` ile yayımlanan sürümü kurup sürümünü denetler. Kurulum betikleri repo kökündedir; README onları `main` dalından indirir.
- Ortam değişkenleri ve test kancaları: [docs/test-hooks.md](docs/test-hooks.md).
- Node yalnız Prettier içindir: `package.json`'daki tek bağımlılık `prettier`; Markdown, YAML ve JSON dosyaları onunla biçimlenir.

## Kurallar

- Araç zinciri `rust-toolchain.toml`'da sabit (1.95). `unsafe` yalnız `gecko-hub-client` `tcp.rs`'teki tek Windows çağrısında, öğe üzerinde `#[allow(unsafe_code)]` ve `SAFETY` yorumuyla; başka yerde `deny`. `clippy` uyarısı hata sayılır.
- TLS `rustls` + `ring` + `rustls-platform-verifier`; `native-tls` ve `aws-lc-rs` girmez. Yeni bağımlılık PR'da gerekçelendirilir.
- `stdout` yalnız sonuç; bilgi `stderr`'e. `--json` ASCII.
- Token yalnız `Authorization` başlığında; asla adreste, SignalR'da da.
- Kullanıcıya giden bütün metinler İngilizce (K2).
- Kod ve kod yorumları İngilizce; commit mesajı ve PR Türkçe. README, CHANGELOG, SECURITY ve `docs/` İngilizce ve kısa (K6). Makine anahtarları İngilizce.

## Dallar ve PR

- `main` tek uzun ömürlü dal; iş PR ile gelir, PR gövdesi Türkçe.
- Force push yok. Commit'ten sonra push.
- AI co-author satırı yok.

## Push etmeden önce

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace
cargo test --workspace --no-default-features
```

Markdown, YAML ya da JSON değiştiyse `pnpm install && pnpm exec prettier --check .` da. İş ortasında yalnız değişen testler; PR öncesi bir kez tam paket.
