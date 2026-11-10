# Ajan rehberi

Bir şeyi değiştirmeden önce oku. Kısa tutuldu; ayrıntı bağlantılı dosyalarda.

## Ne bu repo

`gecko` CLI'ı ve masaüstü CLI spec'inin (frontend-v2-design `design/desktop-cli.md`, 3. taslak) §10.3 sözleşmesini uygulayan geliştirme hub'ı. Spec kanoniktir; buradaki davranış ondan sapıyorsa sapma PR'da ve [CHANGELOG.md](CHANGELOG.md)'de yazılır.

## Düzen

- `src/cli/`: CLI. `main.ts` giriş; `args.ts` seçenekler (komut adından önce, §10.1); `run.ts` bir satırın yolu (§4, §5.5, §6.6); `output.ts` çıktı ve çıkış kodları (§6); `session.ts` bağlantı, bağlama, yetenek listesi; `auth.ts` cihaz girişi; `config.ts` dosyalar ve token izinleri.
- `src/transport/`: `Transport` arayüzü. `dev.ts` geliştirme taşıması (yalnız localhost), `signalr.ts` iskelet (#1974 bekleniyor), `select.ts` seçim.
- `src/hub/`: geliştirme hub'ı, `@dolusoft/gecko-cli/dev-hub` olarak dışa açık.
- `src/tab/`: tarayıcı tarafı istemcisi, `@dolusoft/gecko-cli/tab-client`. Tarayıcıda da çalışır; Node'a özgü modül almaz.
- `src/shared/`: çerçeve ve RPC; tarayıcıda da çalışır.
- `tests/contract/`: hub ve CLI gerçek süreç, sahte sekme command-console-core'un `attachRemote`'u. Spec tablolarının her satırı bir test.
- `tests/unit/`: saf işlevler.

## Rust (geçiş sürüyor)

CLI Rust'a taşınıyor; plan ve kararlar [docs/rust-migration.md](docs/rust-migration.md). Adım 5'e kadar ürün Node kodudur; Rust ikilisi bugün `login`, `logout`, `attach`, `detach` ve `status` yapar (yalnız geliştirme hub'ına karşı), sekmeye satır göndermez.

- `crates/gecko/`: ikili. `args.rs` seçenekler (`args_os`, geçersiz Unicode'da kod 2), `main.rs` yardım, sürüm, `dev-hub` anahtarı ve komut dağıtımı; `own.rs` `login`/`logout`; `binding.rs` `attach`/`detach`/`status`; `session.rs` hub oturumu, kapanış nedeninin çıkış koduna eşlenmesi, eşleştirme; `auth.rs` cihaz girişi, yenileme, iptal; `config.rs` dosyalar (Node ile aynı ad ve JSON: iki ikili birbirinin girişini okur); `output.rs` sonuç ve çıkış kodları; `interrupt.rs` Ctrl+C; `browser.rs`.
- `crates/gecko-wire/`: zarflar ve `--json` için ASCII JSON yazıcısı. Sekmeden okunan enum'lar `#[serde(other)]` taşır.
- `crates/gecko-hub-client/`: `Transport` trait'i (`status`, `pair`, `send`, `detach`, `next_event`), anlamsal `Closed` nedenleri, `HubError`, `HubEvent`. `hub.rs` hub'ın kendi cevapları (`HubStatus`, `Binding`, `PairCode`; bilinmeyen durum `Unknown`). `dev.rs` geliştirme hub'ının JSON çerçeveleri (yalnız localhost); `select.rs` taşıma seçimi (`GECKO_TRANSPORT`); `signalr/` elle yazılmış dar SignalR istemcisi ([docs/signalr-trial.md](docs/signalr-trial.md)); `net.rs` TCP/TLS ve vekil tüneli; `proxy.rs` `*_proxy`/`no_proxy` kuralı.
- `crates/gecko-dev-hub/`: `dev-hub` cargo özelliğinin arkasında (K1). Varsayılan derleme özelliği açar; sürüm derlemesi `--no-default-features` ile yapılır ve "this build has no dev hub" der.
- `tests/signalr-hub/`: SignalR istemci testleri için en küçük ASP.NET Core hub'ı.
- Ortam değişkenleri ve test kancaları: [docs/test-hooks.md](docs/test-hooks.md). Rust aynı adları okur.

Rust kuralları:

- Araç zinciri `rust-toolchain.toml`'da sabit (1.95). `unsafe` yasak. `clippy` uyarısı hata sayılır.
- Kullanıcıya giden bütün metinler İngilizce (K2).
- TLS `rustls` + `ring` + `rustls-platform-verifier`; `native-tls` ve `aws-lc-rs` girmez. Yeni bağımlılık PR'da gerekçelendirilir.
- Token yalnız `Authorization` başlığında; SignalR'da da.

Rust push öncesi:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace
cargo test --workspace --no-default-features
```

## Kurallar

- Çalışma zamanı bağımlılığı yalnız `@dolusoft/command-protocol` ve `ws` (yalnız hub, tembel yüklenir). Küçük paketi kendimiz yazarız.
- CLI'nın açılış yolu ağ ve hub modülü yüklemez; `pnpm measure:startup` `gecko help` için 300 ms sınırını denetler.
- `stdout` yalnız sonuç; bilgi `stderr`'e. `--json` ASCII.
- Token yalnız `Authorization` başlığında; asla adreste.
- Kod ve kod yorumları İngilizce; commit mesajı, PR ve `docs/` Türkçe. README İngilizce ve kısa (K6; Rust ürünüyle, adım 5'te yeniden yazılır). Makine anahtarları İngilizce.

## Dallar ve PR

- `main` tek uzun ömürlü dal; iş PR ile gelir, PR gövdesi Türkçe.
- Force push yok. Commit'ten sonra push.
- AI co-author satırı yok.

## Push etmeden önce

`pnpm lint && pnpm typecheck && pnpm build && pnpm test`; Rust'a dokunulduysa yukarıdaki Rust komutları da. İş ortasında yalnız değişen testler; PR öncesi bir kez tam paket.
