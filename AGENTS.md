# Ajan rehberi

Bir şeyi değiştirmeden önce oku. Kısa tutuldu; ayrıntı bağlantılı dosyalarda.

## Ne bu repo

`gecko` CLI'ı ve masaüstü CLI spec'inin §10.3 sözleşmesini uygulayan geliştirme hub'ı. Spec kanoniktir; buradaki davranış ondan sapıyorsa sapma PR'da yazılır.

Repo public (MIT). Spec ve geçiş planı Dolusoft'un iç belgeleridir, repoda yoktur: kod yorumlarındaki `§` numaraları spec'in, `spec §7.x` geçiş planının, `#1974` Gecko backend'inin hub kaydının, `K1`, `F30` gibi kodlar geçiş planındaki kararların kimliğidir. Repoya iç belge, özel repo bağlantısı ya da müşteri bilgisi girmez.

## Düzen

Ürün Rust ikilisidir (`crates/`). Bugün `attach <code>`, `detach`, `status`, `help` ve sekmeye satır göndermeyi yapar (command-console 1.8.0; yalnız geliştirme hub'ına karşı). Bağlama tek satırdır: sekme bir kerelik `gk1_` kodu üretir, kod hub adresini ve sırrı taşır, CLI onu `POST /cli/attach` ile harcar; hesap girişi ve tarayıcı sayfası yoktur.

- `crates/gecko/`: ikili. `args.rs` seçenekler (`args_os`, geçersiz Unicode'da kod 2), `main.rs` yardım, sürüm, `dev-hub` anahtarı ve komut dağıtımı; `own.rs` kendi komutların adları ve kullanım metinleri; `code.rs` `gk1_` kodunun çözülmesi ve hub adresi kuralı (yalnız `https://` ya da bu bilgisayarda `http://`); `binding.rs` `attach` (güven sorusu, `--trust-hub`, eski bağlamanın bitişi), `detach`, `status`; `help.rs` `gecko help` (`help-request`, sekmenin metni ya da JSON sonucu); `line.rs` sekmeye giden satır (ön süzgeç: hassas ad CLI'dan çıkmaz, listede olmayan addan yalnız ad gider; yetenek listesi, gönderim, bekleyiş); `run.rs` gönderilmiş isteğin saf durum makinesi (cevaplar, kopma, teslim süresi, Ctrl+C girer, sonuç çıkar; `tests/sequence.rs` 1.7.2 ve 1.8.0 `sequence/` örneklerini buna koşar); `session.rs` hub oturumu, kapanış nedeninin çıkış koduna eşlenmesi; `auth.rs` kodun harcanması ve token yenileme (reddedilen yenileme bağlamanın bittiği demektir, token silinir); `config.rs` `token.json`, `config.json` (`trustedHubs`) ve dosya izinleri; `output.rs` sonuç, çıkış kodları, kabuk tırnaklaması, bekleme göstergesi; `format.rs` `data`'nın düz görünümü (tablo, ızgara, `key: value`); `interrupt.rs` Ctrl+C. `tests/binding.rs` ve `tests/line.rs` ikiliyi süreç olarak başlatır, karşısındaki hub ve sekme testin içinde betiklenmiştir; `tests/attach.rs` (yalnız `dev-hub` özelliğiyle) gerçek ikiliyi gerçek geliştirme hub'ına ve sahte bir sekmeye karşı uçtan uca koşar. `tests/e2e.rs` aynısını `gecko-testkit` ile, iki özellik kümesinde koşar; yeni uçtan uca testler buraya yazılır.
- `crates/testkit/` (paket `gecko-testkit`, `publish = false`, yalnız dev-dependency): kara kutu testlerin altyapısı. `hub.rs` geliştirme hub'ını kendi iş parçacığında ve runtime'ında başlatır (hub'ın Rust API'sine dokunan tek dosya; `stop` bütün bağlantıları keser); `tab.rs` sahte sekme (soketi yalnız kendi çağrılarında okur, aradaki sessizlikte heartbeat ping'ine cevap vermez); `envelope.rs` sekme cevaplarının tek kurucusu (her zarf `gecko-wire` okuyucusundan kendi türüyle geçer; `tests/envelopes.rs` 1.8.0 `valid/replies.json` ile alan alan karşılaştırır); `fixtures.rs` 1.8.0 yetenek listesi; `cli.rs` ikili (yolu testten gelir; ortamdan yalnız `GECKO_*` çıkar, kendi config dizini, `NO_COLOR=1`; her beklemenin üst sınırı var); `flow.rs` `connect` ve `disconnect`. `tests/hub.rs` geliştirme hub'ını kara kutu olarak dener: hub testkit'in `Hub`'ıyla kurulur, test onunla yalnız HTTP ve WebSocket üzerinden konuşur.
- `crates/gecko-wire/`: sekmeyle sözleşme (command-console 1.8.0) ve `--json` için ASCII JSON yazıcısı. `reply.rs` sekmenin cevaplarını alan alan okur (bilinmeyen `kind` atlanır, bilinmeyen `status`/`class` tanımlı varsayılana düşer); `request.rs` komut adı kuralı, gönderim öncesi sınırlar ve zarflar; `capabilities.rs` parçalı yetenek listesi, gereken `features`, `minCli`, `change` komutunun `effect` alanı (bilinmeyen değer `server`). `finished` cevabının `approval` alanı `card` ya da `none` okunur (bilinmeyen değer `none`). `tests/fixtures.rs` `fixtures/command-console-1.7.2/` ve `fixtures/command-console-1.8.0/` örneklerini koşar; bu klasörler yayın ekinden olduğu gibi kopyalanır, elle düzenlenmez ([fixtures/README.md](fixtures/README.md)).
- `crates/gecko-hub-client/`: `Transport` trait'i (`status`, `send`, `detach`, `next_event`), anlamsal `Closed` nedenleri, `HubError`, `HubEvent`. `hub.rs` hub'ın kendi cevapları (`HubStatus`, `Binding` ve onay seviyesi; bilinmeyen durum `Unknown`). `dev.rs` geliştirme hub'ının JSON çerçeveleri (yalnız localhost); `select.rs` taşıma seçimi (`GECKO_TRANSPORT`); `signalr/` elle yazılmış dar SignalR istemcisi (hazır crate token'ı adreste taşıdığı için); `tcp.rs` hızlı reddedilen loopback bağlantısı (Windows, tek `unsafe`); `net.rs` TCP/TLS ve vekil tüneli; `proxy.rs` `*_proxy`/`no_proxy` kuralı.
- `crates/gecko-dev-hub/`: `gecko dev-hub` (K1). `hub.rs` kayıtlar ve kurallar (bağlama kodu: hub sırrın yalnız SHA-256 özetini tutar; kodun harcanması, adres başına yanlış deneme sınırı, token, bağlama ve onay seviyesi, sayfa yenilemesinde `unload`/`resume`, askıdaki bağlamaya giden `send`'in bekletilmesi, damga, hız sınırı, heartbeat); `server.rs` HTTP/1.1 ve `/hub` WebSocket yükseltmesi, bağlantı başına tek istek; `random.rs` sırlar ve base64url. `dev-hub` cargo özelliğinin arkasında: varsayılan derleme özelliği açar, sürüm derlemesi `--no-default-features` ile yapılır ve "this build has no dev hub" der; `sha2` yalnız bu crate'te olduğundan sürüm ikilisine girmez. Hub'ın testleri bu crate'te değil, `crates/testkit/tests/hub.rs`'te: testkit hub'a bağlı olduğundan hub'ın testkit'e dev-dependency'si paket grafiğinde döngü kurardı.
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
