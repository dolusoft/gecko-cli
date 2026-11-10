# Ajan rehberi

Bir şeyi değiştirmeden önce oku. Kısa tutuldu; ayrıntı bağlantılı dosyalarda.

## Ne bu repo

`gecko` CLI'ı ve masaüstü CLI spec'inin §10.3 sözleşmesini uygulayan geliştirme hub'ı. Spec kanoniktir; buradaki davranış ondan sapıyorsa sapma PR'da yazılır.

Repo public (MIT). Spec ve geçiş planı Dolusoft'un iç belgeleridir, repoda yoktur: kod yorumlarındaki `§` numaraları spec'in, `spec §7.x` geçiş planının, `#1974` Gecko backend'inin hub kaydının, `K1`, `F30` gibi kodlar geçiş planındaki kararların kimliğidir. Repoya iç belge, özel repo bağlantısı ya da müşteri bilgisi girmez.

## Düzen

Ürün Rust ikilisidir (`crates/`). Bugün `attach <code>`, `detach`, `status`, `help` ve sekmeye satır göndermeyi yapar (command-console 1.8.0; yalnız geliştirme hub'ına karşı). Bağlama tek satırdır: sekme bir kerelik `gk1_` kodu üretir, kod hub adresini ve sırrı taşır, CLI onu `POST /cli/attach` ile harcar; hesap girişi ve tarayıcı sayfası yoktur.

- `crates/app/` (paket `gecko`): ikili. `args.rs` seçenekler (`args_os`, geçersiz Unicode'da kod 2), `main.rs` yardım, sürüm, `dev-hub` anahtarı, komut dağıtımı ve bileşim kökü (ortam yalnız burada okunur: `Settings`, vekil kuralı, config dizini, `NO_COLOR`, test kancaları; port'lar burada kurulur); `ports.rs` `Clock` ve `IdSource` uygulamaları (`SystemClock`, `RandomIds`) ve bir çağrının değerleri (`Ports`: `Settings`, saat, kimlikler, CLI sürümü, vekil kuralı); `own.rs` kendi komutların adları ve kullanım metinleri; `code.rs` `gk1_` kodunun çözülmesi ve hub adresi kuralı (yalnız `https://` ya da bu bilgisayarda `http://`); `binding.rs` `attach` (güven sorusu, `--trust-hub`, eski bağlamanın bitişi), `detach`, `status`; `command.rs` `command` bağlamının ikilideki ucu (satırın girdisi: argv, `--line`, `--line -` ile stdin; bağlantıyı açıp `gecko-command-module`'e verir, sonra kapatır); `session.rs` hub oturumu (bağlantının açılması, `Session<H>`; kapanış nedeninin hataya eşlenmesi `gecko-hub-protocol`'de); `auth.rs` kodun harcanması ve token yenileme (reddedilen yenileme bağlamanın bittiği demektir, token silinir); `config.rs` `token.json`, `config.json` (`trustedHubs`) ve dosya izinleri; `output.rs` sonucun yazılması (`Io`, renk; `Io` `Reporter`'ı uygular), kabuk tırnaklaması, bekleme göstergesi; `format.rs` `data`'nın düz görünümü (tablo, ızgara, `key: value`); `interrupt.rs` Ctrl+C (`CtrlC`, `Interrupts`'ı uygular). `tests/exit_codes.rs` `exit_code_of` tablo testi ve CLI'ın yazdığı her hata kodunun tabloda olduğunu denetleyen tarama; taranan kaynak klasörleri tek listede (`SOURCE_ROOTS`), bağlam crate'leri ona eklenir. `tests/binding.rs` ve `tests/line.rs` ikiliyi süreç olarak başlatır, karşısındaki hub ve sekme testin içinde betiklenmiştir; `tests/attach.rs` (yalnız `dev-hub` özelliğiyle) gerçek ikiliyi gerçek geliştirme hub'ına ve sahte bir sekmeye karşı uçtan uca koşar. `tests/e2e.rs` aynısını `gecko-testkit` ile, iki özellik kümesinde koşar; yeni uçtan uca testler buraya yazılır. `tests/architecture.rs` mimari testi (aşağıda "Mimari").
- `crates/testkit/` (paket `gecko-testkit`, `publish = false`, yalnız dev-dependency): kara kutu testlerin altyapısı. `hub.rs` geliştirme hub'ını kendi iş parçacığında ve runtime'ında başlatır (hub'ın Rust API'sine dokunan tek dosya; `stop` bütün bağlantıları keser); `tab.rs` sahte sekme (soketi yalnız kendi çağrılarında okur, aradaki sessizlikte heartbeat ping'ine cevap vermez); `envelope.rs` sekme cevaplarının tek kurucusu (her zarf `gecko-wire` okuyucusundan kendi türüyle geçer; `tests/envelopes.rs` 1.8.2 `valid/replies.json` ile alan alan karşılaştırır); `fixtures.rs` örnek dosyalarının okuyucusu ve 1.8.2 örneklerinin yetenek listesi; `cli.rs` ikili (yolu testten gelir; ortamdan yalnız `GECKO_*` çıkar, kendi config dizini, `NO_COLOR=1`; her beklemenin üst sınırı var); `flow.rs` `connect` ve `disconnect`. `tests/hub.rs` geliştirme hub'ını kara kutu olarak dener: hub testkit'in `Hub`'ıyla kurulur, test onunla yalnız HTTP ve WebSocket üzerinden konuşur.
- `crates/protocol/wire/` (paket `gecko-wire`): sekmeyle sözleşme (command-console 1.8.0) ve `--json` için ASCII JSON yazıcısı. `reply.rs` sekmenin cevaplarını alan alan okur (bilinmeyen `kind` atlanır, bilinmeyen `status`/`class` tanımlı varsayılana düşer); `request.rs` komut adı kuralı, gönderim öncesi sınırlar ve zarflar; `capabilities.rs` parçalı yetenek listesi, gereken `features`, `minCli`, `change` komutunun `effect` alanı (bilinmeyen değer `server`). `finished` cevabının `approval` alanı `card` ya da `none` okunur (bilinmeyen değer `none`). `tests/fixtures.rs` `fixtures/command-console-1.7.2/` ve `fixtures/command-console-1.8.2/` örneklerini koşar; bu klasörler yayın ekinden olduğu gibi kopyalanır, elle düzenlenmez ([fixtures/README.md](fixtures/README.md)). Testler örnek klasörünü `GECKO_CLI_FIXTURES_DIR` ile bulur: `.cargo/config.toml` onu mutlak yol olarak her derlemeye verir, testler `env!` ile okur; crate'in derinliği değişince yol kırılmaz. CLI bu değişkeni okumaz.
- `crates/kernel/` (paket `gecko-kernel`): ortak çekirdek, I/O yok. `outcome.rs` `Outcome` (`--json` sonucu) ve `ViewInfo`; `failure.rs` `Failure` ve `exit_code_of` (CLI'ın kendi hata kodlarının çıkış kodu); `messages.rs` birden çok komutun paylaştığı metinler (`BIND_AGAIN`, `gone_message`, `no_binding`); `ports.rs` port trait'leri (`Reporter`: stderr satırı, soru, bekleme göstergesi, `json`, `columns`; `Interrupts`; `Clock`; `IdSource`) ve `Settings` değeri (`GECKO_TRANSPORT`, `GECKO_TEST_TTY`, `GECKO_DELIVERY_MARGIN_MS`, `GECKO_CANCEL_WAIT_MS`; ikili ortamdan okur). `clippy.toml` saati, ortamı, standart akışları, dosya, ağ ve süreç çağrılarını yasaklar.
- `crates/protocol/hub/` (paket `gecko-hub-protocol`): hub ile sözleşme, bağlantısız. `lib.rs` anlamsal `Closed` nedenleri (kapatma kodlarının eşlenmesi), `HubError`, `CallError`, `HubEvent`; `hub.rs` hub'ın kendi cevapları (`HubStatus`, `Binding` ve onay seviyesi; bilinmeyen durum `Unknown`); `address.rs` adres kuralları (`is_local_host`, `choose`: taşıma seçimi, `GECKO_TRANSPORT` değeri parametre olarak gelir); `connection.rs` `HubConnection` port'u (`status`, `send`, `detach`, `next_event`, `close`; generic kullanılır, `dyn` değil); `failure.rs` `Closed` ve `CallError`'ın `Failure`'a eşlenmesi (`open_failure`, `lost_failure`, `call_failure`). `gecko-kernel`'e bağlıdır.
- `crates/adapters/hub-client/` (paket `gecko-hub-client`): taşımalar. Sözleşme tiplerini ve `HubConnection` port'unu `gecko-hub-protocol`'den eski yollarıyla yeniden dışa aktarır (`hub`, `select`, `Closed`, `HubError`, `CallError`, `HubEvent`, `Transport`). `dev.rs` geliştirme hub'ının JSON çerçeveleri (yalnız localhost; `HubConnection`'ı uygular); `signalr/` elle yazılmış dar SignalR istemcisi (hazır crate token'ı adreste taşıdığı için; `HubConnection`'ı henüz uygulamaz, CLI onu açmaz); `tcp.rs` hızlı reddedilen loopback bağlantısı (Windows, tek `unsafe`); `net.rs` TCP/TLS ve vekil tüneli; `proxy.rs` `*_proxy`/`no_proxy` kuralı.
- `crates/contexts/command/` (`command` bağlamı: sekmeye giden satır ve `gecko help`). `contract/` (paket `gecko-command-contract`): bir çağrının bağlama verdiği değerler (`Ports`: `Settings`, `Clock`, `IdSource`, CLI sürümü). `domain/` (paket `gecko-command-domain`, I/O ve saat yok, `clippy.toml` ile): `line.rs` kurallar (`precheck`: ad kuralı ve sınırlar; ön süzgeç `pre_filter`: hassas ad CLI'dan çıkmaz, listede olmayan addan yalnız ad gider; `capabilities_of` ve `minCli`, sürüm değer olarak gelir; `live_binding`; `Stamp::delivery_window` teslim süresi); `run.rs` gönderilmiş isteğin saf durum makinesi (cevaplar, kopma, teslim süresi, Ctrl+C girer, sonuç çıkar; saat parametredir, `Run::on(event, now)`; `tests/sequence.rs` 1.7.2 ve 1.8.2 `sequence/` örneklerini buna koşar); `help.rs` `help-result`'ın sonuç biçimi ve `help …` satırı. `module/` (paket `gecko-command-module`): `exchange.rs` iki dilimin ortak işi (`send`, hız sınırında yeniden deneme, cevapların kimlikle süzülmesi, `help-request` ve bekleyişi); `features/run_line.rs` satırın gönderimi ve bekleyişi (monoton zamanlayıcılar burada); `features/help.rs` `gecko help`. Hub'a generic `HubConnection`, ekrana `Reporter`, Ctrl+C'ye `Interrupts` ile erişir; bağlantıyı açmak ikilinin işi.
- `crates/dev-hub/` (paket `gecko-dev-hub`): `gecko dev-hub` (K1). `hub.rs` kayıtlar ve kurallar (bağlama kodu: hub sırrın yalnız SHA-256 özetini tutar; kodun harcanması, adres başına yanlış deneme sınırı, token, bağlama ve onay seviyesi, sayfa yenilemesinde `unload`/`resume`, askıdaki bağlamaya giden `send`'in bekletilmesi, damga, hız sınırı, heartbeat); `server.rs` HTTP/1.1 ve `/hub` WebSocket yükseltmesi, bağlantı başına tek istek; `random.rs` sırlar ve base64url. `dev-hub` cargo özelliğinin arkasında: varsayılan derleme özelliği açar, sürüm derlemesi `--no-default-features` ile yapılır ve "this build has no dev hub" der; `sha2` yalnız bu crate'te olduğundan sürüm ikilisine girmez. Hub'ın testleri bu crate'te değil, `crates/testkit/tests/hub.rs`'te: testkit hub'a bağlı olduğundan hub'ın testkit'e dev-dependency'si paket grafiğinde döngü kurardı.
- `tests/signalr-hub/`: SignalR istemci testleri için en küçük ASP.NET Core hub'ı (CI `signalr` işi).
- Yayın: `.github/workflows/release.yml`. Akış: `Cargo.toml`'daki `version` ve `CHANGELOG.md`'deki `## <sürüm>` bölümü aynı PR'da yazılır. Merge'den sonra iş akışı elle çalıştırılır; bu deneme yalnız derler. Sonra `v<sürüm>` tag'i atılır. Tag altı hedefi `--no-default-features` ile derler, Release'i `SHA256SUMS` ve CHANGELOG bölümüyle açar, sonra `install.sh` / `install.ps1` ile yayımlanan sürümü kurup sürümünü denetler. Kurulum betikleri repo kökündedir; README onları `main` dalından indirir.
- Ortam değişkenleri ve test kancaları: [docs/test-hooks.md](docs/test-hooks.md).
- Node yalnız Prettier içindir: `package.json`'daki tek bağımlılık `prettier`; Markdown, YAML ve JSON dosyaları onunla biçimlenir.

Workspace üyeleri kök `Cargo.toml`'da açık bir listedir (`crates/*` deseni iç içe klasörlerde kırılır); paket adları klasör adından bağımsızdır.

## Mimari

`crates/app/tests/architecture.rs` her crate'in rolünü ve kimin kime bağlanabileceğini denetler. Kontroller `cargo metadata` çıktısı, manifest ve kaynak metni üzerinde saf işlevlerdir; her birinin bozuk girdiyle kırmızı veren birim testi vardır. Yeni crate testteki `ROLES` listesine bir rolle girmeden test kırmızıdır.

| Rol            | Crate'ler (bugün)                  | Bağlanabilir                                                                                     |
| -------------- | ---------------------------------- | ------------------------------------------------------------------------------------------------ |
| `kernel`       | `gecko-kernel`                     | yalnız izinli dış crate'ler (`serde`, `serde_json`)                                              |
| `protocol`     | `gecko-wire`, `gecko-hub-protocol` | `kernel`, izinli dış crate'ler                                                                   |
| `contract`     | `gecko-command-contract`           | `kernel`, `protocol`                                                                             |
| `domain`       | `gecko-command-domain`             | kendi `contract`'ı, `kernel`, `protocol`; `clippy.toml` kopyası zorunlu                          |
| `module`       | `gecko-command-module`             | kendi `domain`'i, `contract`, `kernel`, `protocol`, `adapter-lib`; I/O `src/adapters/**` altında |
| `adapter-lib`  | `gecko-hub-client`                 | `kernel`, `protocol`                                                                             |
| `dev-tool`     | `gecko-dev-hub`                    | workspace crate'i yok; ona yalnız ikili (özellik arkasında) ve `test-support` bağlanır           |
| `test-support` | `gecko-testkit`                    | herkes; yalnız `dev-dependencies` içinde görünür                                                 |
| `app`          | yok                                | herkes (`test-support` yalnız dev)                                                               |
| `legacy`       | `gecko`                            | herkes; geçici, liste yalnız kısalır                                                             |

Ayrıca: her manifest `[lints] workspace = true` taşır ve kök lint tablosu testtekiyle aynıdır; hiçbir crate kaynağı yol ile içeri almaz (bekleyen istisna yok); `--no-default-features` ile çözülen `gecko` grafiğinde `gecko-dev-hub`, `sha2` ve `gecko-testkit` yoktur.

**Kapı dosyaları:** `crates/app/tests/architecture.rs`, bütün `clippy.toml` dosyaları ve kök `Cargo.toml`'un `[workspace.lints.*]` bölümü. Bunlara dokunan değişiklik ayrı bir PR'dır; gerekçesi PR açıklamasında yazar.

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
