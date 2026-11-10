# Ajan rehberi

Bir şeyi değiştirmeden önce oku. Kısa tutuldu; ayrıntı bağlantılı dosyalarda.

## Ne bu repo

`gecko` CLI'ı ve masaüstü CLI spec'inin §10.3 sözleşmesini uygulayan geliştirme hub'ı. Spec kanoniktir; buradaki davranış ondan sapıyorsa sapma PR'da yazılır.

Repo public (MIT). Spec ve geçiş planı Dolusoft'un iç belgeleridir, repoda yoktur: kod yorumlarındaki `§` numaraları spec'in, `spec §7.x` geçiş planının, `#1974` Gecko backend'inin hub kaydının, `K1`, `F30` gibi kodlar geçiş planındaki kararların kimliğidir. Repoya iç belge, özel repo bağlantısı ya da müşteri bilgisi girmez.

## Düzen

- `src/cli/`: CLI. `main.ts` giriş; `args.ts` seçenekler (komut adından önce, §10.1); `run.ts` bir satırın yolu (§4, §5.5, §6.6); `output.ts` çıktı ve çıkış kodları (§6); `session.ts` bağlantı, bağlama, yetenek listesi; `auth.ts` cihaz girişi; `config.ts` dosyalar ve token izinleri.
- `src/transport/`: `Transport` arayüzü. `dev.ts` geliştirme taşıması (yalnız localhost), `signalr.ts` iskelet (#1974 bekleniyor), `select.ts` seçim.
- `src/hub/`: geliştirme hub'ı, `@dolusoft/gecko-cli/dev-hub` olarak dışa açık.
- `src/tab/`: tarayıcı tarafı istemcisi, `@dolusoft/gecko-cli/tab-client`. Tarayıcıda da çalışır; Node'a özgü modül almaz.
- `src/shared/`: çerçeve ve RPC; tarayıcıda da çalışır.
- `tests/contract/`: hub ve CLI gerçek süreç, sahte sekme command-console-core'un `attachRemote`'u. Spec tablolarının her satırı bir test.
- `tests/unit/`: saf işlevler.

## Rust (geçiş sürüyor)

CLI Rust'a taşınıyor (plan iç belgede). Adım 5'e kadar ürün Node kodudur; Rust ikilisi bugün `login`, `logout`, `attach`, `detach`, `status`, `help` ve sekmeye satır göndermeyi yapar (command-console 1.7.2; yalnız geliştirme hub'ına karşı).

- `crates/gecko/`: ikili. `args.rs` seçenekler (`args_os`, geçersiz Unicode'da kod 2), `main.rs` yardım, sürüm, `dev-hub` anahtarı ve komut dağıtımı; `own.rs` `login`/`logout`; `binding.rs` `attach`/`detach`/`status`; `help.rs` `gecko help` (`help-request`, sekmenin metni ya da JSON sonucu); `line.rs` sekmeye giden satır (ön süzgeç: hassas ad CLI'dan çıkmaz, listede olmayan addan yalnız ad gider; yetenek listesi, gönderim, bekleyiş); `run.rs` gönderilmiş isteğin saf durum makinesi (cevaplar, kopma, teslim süresi, Ctrl+C girer, sonuç çıkar; `tests/sequence.rs` 1.7.0 `sequence/` örneklerini buna koşar); `session.rs` hub oturumu, kapanış nedeninin çıkış koduna eşlenmesi, eşleştirme; `auth.rs` cihaz girişi, yenileme, iptal; `config.rs` dosyalar (Node ile aynı ad ve JSON: iki ikili birbirinin girişini okur); `output.rs` sonuç, çıkış kodları, kabuk tırnaklaması, bekleme göstergesi; `format.rs` `data`'nın düz görünümü (tablo, ızgara, `key: value`); `interrupt.rs` Ctrl+C; `browser.rs`.
- `crates/gecko-wire/`: sekmeyle sözleşme (command-console 1.7.2) ve `--json` için ASCII JSON yazıcısı. `reply.rs` sekmenin cevaplarını alan alan okur (bilinmeyen `kind` atlanır, bilinmeyen `status`/`class` tanımlı varsayılana düşer); `request.rs` komut adı kuralı, gönderim öncesi sınırlar ve zarflar; `capabilities.rs` parçalı yetenek listesi, gereken `features`, `minCli`. `tests/fixtures.rs` `fixtures/command-console-1.7.2/` örneklerini koşar; o klasör yayın ekinden olduğu gibi kopyalanır, elle düzenlenmez ([fixtures/README.md](fixtures/README.md)).
- `crates/gecko-hub-client/`: `Transport` trait'i (`status`, `pair`, `send`, `detach`, `next_event`), anlamsal `Closed` nedenleri, `HubError`, `HubEvent`. `hub.rs` hub'ın kendi cevapları (`HubStatus`, `Binding`, `PairCode`; bilinmeyen durum `Unknown`). `dev.rs` geliştirme hub'ının JSON çerçeveleri (yalnız localhost); `select.rs` taşıma seçimi (`GECKO_TRANSPORT`); `signalr/` elle yazılmış dar SignalR istemcisi (hazır crate token'ı adreste taşıdığı için); `tcp.rs` hızlı reddedilen loopback bağlantısı (Windows, tek `unsafe`); `net.rs` TCP/TLS ve vekil tüneli; `proxy.rs` `*_proxy`/`no_proxy` kuralı.
- `crates/gecko-dev-hub/`: `gecko dev-hub`, Node hub'ının (`src/hub/hub.ts`) birebir karşılığı (K1). `hub.rs` kayıtlar ve kurallar (cihaz girişi, token, bağlama, damga, hız sınırı, heartbeat); `server.rs` HTTP/1.1 ve `/hub` WebSocket yükseltmesi, bağlantı başına tek istek; `page.rs` `/cli-login` sayfası; `random.rs` kod ve sırlar. `dev-hub` cargo özelliğinin arkasında: varsayılan derleme özelliği açar, sürüm derlemesi `--no-default-features` ile yapılır ve "this build has no dev hub" der. Node sözleşme testleri `GECKO_TEST_HUB_BIN=<gecko ikilisi>` ile Rust hub'ına karşı koşar (CI `hub` işi); davranış değişirse iki hub birlikte değişir.
- `tests/signalr-hub/`: SignalR istemci testleri için en küçük ASP.NET Core hub'ı.
- Ortam değişkenleri ve test kancaları: [docs/test-hooks.md](docs/test-hooks.md). Rust aynı adları okur.

Rust kuralları:

- Araç zinciri `rust-toolchain.toml`'da sabit (1.95). `unsafe` yalnız `gecko-hub-client` `tcp.rs`'teki tek Windows çağrısında, öğe üzerinde `#[allow(unsafe_code)]` ve `SAFETY` yorumuyla; başka yerde `deny`. `clippy` uyarısı hata sayılır.
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
- Kod ve kod yorumları İngilizce; commit mesajı ve PR Türkçe. README, CHANGELOG, SECURITY ve `docs/` İngilizce ve kısa (K6). Makine anahtarları İngilizce.

## Dallar ve PR

- `main` tek uzun ömürlü dal; iş PR ile gelir, PR gövdesi Türkçe.
- Force push yok. Commit'ten sonra push.
- AI co-author satırı yok.

## Push etmeden önce

`pnpm lint && pnpm typecheck && pnpm build && pnpm test`; Rust'a dokunulduysa yukarıdaki Rust komutları da. İş ortasında yalnız değişen testler; PR öncesi bir kez tam paket.
