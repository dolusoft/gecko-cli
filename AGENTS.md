# Ajan rehberi

Bir şeyi değiştirmeden önce oku. Kısa tutuldu; ayrıntı bağlantılı dosyalarda.

## Ne bu repo

`gecko` CLI'ı ve masaüstü CLI spec'inin §10.3 sözleşmesini uygulayan geliştirme hub'ı. Spec kanoniktir; buradaki davranış ondan sapıyorsa sapma PR'da yazılır.

Repo public (MIT). Spec ve geçiş planı Dolusoft'un iç belgeleridir, repoda yoktur: kod yorumlarındaki `§` numaraları spec'in, `spec §7.x` geçiş planının, `#1974` Gecko backend'inin hub kaydının, `K1`, `F30` gibi kodlar geçiş planındaki kararların kimliğidir. Repoya iç belge, özel repo bağlantısı ya da müşteri bilgisi girmez.

## Düzen

Ürün Rust ikilisidir (`crates/`). Bugün `attach <code>`, `detach`, `status`, `help` ve sekmeye satır göndermeyi yapar (command-console 1.8.2; yalnız geliştirme hub'ına karşı). Bağlama tek satırdır: sekme bir kerelik `gk1_` kodu üretir, kod hub adresini ve sırrı taşır, CLI onu `POST /cli/attach` ile harcar; hesap girişi ve tarayıcı sayfası yoktur.

- `crates/app/` (paket `gecko`): ikili. `args.rs` seçenekler (`args_os`, geçersiz Unicode'da kod 2), `main.rs` yardım, sürüm, `dev-hub` anahtarı, komut dağıtımı ve bileşim kökü (ortam yalnız burada okunur: `Settings`, vekil kuralı, config dizini, `NO_COLOR`, test kancaları; port'lar ve token deposu `Config` burada kurulur, ikilinin geri kalanı depoyu yalnız `TokenStore` port'u olarak görür); `ports.rs` `Clock` ve `IdSource` uygulamaları (`SystemClock`, `RandomIds`) ve bir çağrının değerleri (`Ports`: `Settings`, saat, kimlikler, CLI sürümü, vekil kuralı); `own.rs` kendi komutların adları ve kullanım metinleri; `binding.rs` `binding` bağlamının ikilideki ucu (`attach` argümanları, `attach -` ile stdin'den kod, terminal mi kararı; port uygulamaları: güven sorusunun cevabı stdin'den `StdinPrompt`, bağlantıyı açan `Opener`); `command.rs` `command` bağlamının ikilideki ucu (satırın girdisi: argv, `--line`, `--line -` ile stdin; bağlantıyı açıp `gecko-command-module`'e verir, sonra kapatır); `session.rs` hub bağlantısının açılması (token, taşıma seçimi, 401'de bir kez yenileme `gecko-binding-module`'ün `adapters::http::Auth`'u ile, `status`; `Session<H>` değeri ve kapanış nedeninin hataya eşlenmesi `gecko-hub-protocol`'de); `output.rs` sonucun yazılması (`Io`, renk; `Io` `Reporter`'ı uygular), kabuk tırnaklaması, bekleme göstergesi; `format.rs` `data`'nın düz görünümü (tablo, ızgara, `key: value`); `interrupt.rs` Ctrl+C (`CtrlC`, `Interrupts`'ı uygular). `tests/exit_codes.rs` `exit_code_of` tablo testi ve CLI'ın yazdığı her hata kodunun tabloda olduğunu denetleyen tarama; taranan kaynak klasörleri tek listede (`SOURCE_ROOTS`), bağlam crate'leri ona eklenir. `tests/binding.rs` ve `tests/line.rs` ikiliyi süreç olarak başlatır, karşısındaki hub ve sekme testin içinde betiklenmiştir; `tests/attach.rs` (yalnız `dev-hub` özelliğiyle) gerçek ikiliyi gerçek geliştirme hub'ına ve sahte bir sekmeye karşı uçtan uca koşar. `tests/e2e.rs` aynısını `gecko-testkit` ile, iki özellik kümesinde koşar; yeni uçtan uca testler buraya yazılır. `tests/architecture.rs` mimari testi (aşağıda "Mimari").
- `crates/testkit/` (paket `gecko-testkit`, `publish = false`, yalnız dev-dependency): kara kutu testlerin altyapısı. `hub.rs` geliştirme hub'ını kendi iş parçacığında ve runtime'ında başlatır (hub'ın Rust API'sine dokunan tek dosya; `stop` bütün bağlantıları keser); `tab.rs` sahte sekme (soketi yalnız kendi çağrılarında okur, aradaki sessizlikte heartbeat ping'ine cevap vermez); `envelope.rs` sekme cevaplarının tek kurucusu (her zarf `gecko-wire` okuyucusundan kendi türüyle geçer; `tests/envelopes.rs` 1.8.2 `valid/replies.json` ile alan alan karşılaştırır); `fixtures.rs` örnek dosyalarının okuyucusu ve 1.8.2 örneklerinin yetenek listesi; `cli.rs` ikili (yolu testten gelir; ortamdan yalnız `GECKO_*` çıkar, kendi config dizini, `NO_COLOR=1`; her beklemenin üst sınırı var); `flow.rs` `connect` ve `disconnect`. `tests/hub.rs` geliştirme hub'ını kara kutu olarak dener: hub testkit'in `Hub`'ıyla kurulur, test onunla yalnız HTTP ve WebSocket üzerinden konuşur.
- `crates/protocol/wire/` (paket `gecko-wire`): sekmeyle sözleşme (command-console 1.8.2) ve `--json` için ASCII JSON yazıcısı. `reply.rs` sekmenin cevaplarını alan alan okur (bilinmeyen `kind` atlanır, bilinmeyen `status`/`class` tanımlı varsayılana düşer); `request.rs` komut adı kuralı, gönderim öncesi sınırlar ve zarflar; `capabilities.rs` parçalı yetenek listesi, gereken `features`, `minCli`, `change` komutunun `effect` alanı (bilinmeyen değer `server`). `finished` cevabının `approval` alanı `card` ya da `none` okunur (bilinmeyen değer `none`). Modüller özeldir, her ad crate kökünden dışa aktarılır (`gecko_wire::Reply`). `gecko-testkit`'in `tests/wire_fixtures.rs`'i `fixtures/command-console-1.7.2/` ve `fixtures/command-console-1.8.2/` örneklerini bu crate'e koşar (dosya okumak I/O, `protocol` crate'inde yok); bu klasörler yayın ekinden olduğu gibi kopyalanır, elle düzenlenmez ([fixtures/README.md](fixtures/README.md)). Testler örnek klasörünü `GECKO_CLI_FIXTURES_DIR` ile bulur: `.cargo/config.toml` onu mutlak yol olarak her derlemeye verir, testler `env!` ile okur; crate'in derinliği değişince yol kırılmaz. CLI bu değişkeni okumaz.
- `crates/kernel/` (paket `gecko-kernel`): ortak çekirdek, I/O yok. `outcome.rs` `Outcome` (`--json` sonucu) ve `ViewInfo`; `failure.rs` `Failure` ve `exit_code_of` (CLI'ın kendi hata kodlarının çıkış kodu); `messages.rs` birden çok komutun paylaştığı metinler (`BIND_AGAIN`, `gone_message`, `no_binding`); `ports.rs` port trait'leri (`Reporter`: stderr satırı, soru, bekleme göstergesi, `json`, `columns`; `Interrupts`; `Clock`; `IdSource`) ve `Settings` değeri (`GECKO_TRANSPORT`, `GECKO_TEST_TTY`, `GECKO_DELIVERY_MARGIN_MS`, `GECKO_CANCEL_WAIT_MS`; ikili ortamdan okur). `clippy.toml` saati, ortamı, standart akışları, dosya, ağ ve süreç çağrılarını yasaklar.
- `crates/protocol/hub/` (paket `gecko-hub-protocol`): hub ile sözleşme, bağlantısız. Modüller özeldir, her ad crate kökünden dışa aktarılır (`gecko_hub_protocol::HubStatus`). `lib.rs` anlamsal `Closed` nedenleri (kapatma kodlarının eşlenmesi), `HubError`, `CallError`, `HubEvent`; `hub.rs` hub'ın kendi cevapları (`HubStatus`, `Binding` ve onay seviyesi; bilinmeyen durum `Unknown`); `address.rs` adres kuralları (`is_local_host`, `choose`: taşıma seçimi, `GECKO_TRANSPORT` değeri parametre olarak gelir); `connection.rs` `HubConnection` port'u (`status`, `send`, `detach`, `next_event`, `close`; generic kullanılır, `dyn` değil); `session.rs` `Session<H>` (açık bağlantı ve hub'ın `status`'u; bir değer, bağlantıyı açmak ikilinin işi); `failure.rs` `Closed` ve `CallError`'ın `Failure`'a eşlenmesi (`open_failure`, `lost_failure`, `call_failure`). `gecko-kernel`'e bağlıdır.
- `crates/adapters/hub-client/` (paket `gecko-hub-client`): taşımalar. Sözleşme tiplerini ve `HubConnection` port'unu `gecko-hub-protocol`'den yeniden dışa aktarır (`Closed`, `HubError`, `CallError`, `HubEvent`, `Transport`). `dev.rs` geliştirme hub'ının JSON çerçeveleri (yalnız localhost; `HubConnection`'ı uygular); `signalr/` elle yazılmış dar SignalR istemcisi (hazır crate token'ı adreste taşıdığı için; `HubConnection`'ı henüz uygulamaz, CLI onu açmaz); `tcp.rs` hızlı reddedilen loopback bağlantısı (Windows, tek `unsafe`); `net.rs` TCP/TLS ve vekil tüneli; `proxy.rs` `*_proxy`/`no_proxy` kuralı.
- `crates/contexts/command/` (`command` bağlamı: sekmeye giden satır ve `gecko help`). `contract/` (paket `gecko-command-contract`): bir çağrının bağlama verdiği değerler (`Ports`: `Settings`, `Clock`, `IdSource`, CLI sürümü). `domain/` (paket `gecko-command-domain`, I/O ve saat yok, `clippy.toml` ile): `line.rs` kurallar (`precheck`: ad kuralı ve sınırlar; ön süzgeç `pre_filter`: hassas ad CLI'dan çıkmaz, listede olmayan addan yalnız ad gider; `capabilities_of` ve `minCli`, sürüm değer olarak gelir; `live_binding`; `Stamp::delivery_window` teslim süresi); `run.rs` gönderilmiş isteğin saf durum makinesi (cevaplar, kopma, teslim süresi, Ctrl+C girer, sonuç çıkar; saat parametredir, `Run::on(event, now)`; `crates/testkit/tests/sequence.rs` 1.7.2 ve 1.8.2 `sequence/` örneklerini buna koşar; dosya okumak I/O olduğu için test domain'de değil); `help.rs` `help-result`'ın sonuç biçimi ve `help …` satırı. `module/` (paket `gecko-command-module`): `exchange.rs` iki dilimin ortak işi (`send`, hız sınırında yeniden deneme, cevapların kimlikle süzülmesi, `help-request` ve bekleyişi); `features/run_line.rs` satırın gönderimi ve bekleyişi (monoton zamanlayıcılar burada); `features/help.rs` `gecko help`. Modülün dışa açtığı yalnız `run_line`, `help`, `Ports` ve bağlantı açılmadan önce gereken iki kural (`precheck`, `line_args`; domain'i saran işlevler, imzalarında yalnız `wire` ve `kernel` türleri); ikili domain'e doğrudan bağlanmaz. Hub'a generic `HubConnection` (`Session<H>` ile), ekrana `Reporter`, Ctrl+C'ye `Interrupts` ile erişir; bağlantıyı açmak ikilinin işi.
- `crates/contexts/binding/` (`binding` bağlamı: `attach`, `detach`, `status`, token ve deposu). `contract/` (paket `gecko-binding-contract`): `StoredToken` ve `User`; port'lar `TokenStore` (token oku, yaz, sil; onaylı hub adresleri), `TrustPrompt` (güven sorusunun cevabı; soru `Reporter::prompt` ile gider), `HubOpener` (token ile bağlantı açar, `Session<H>` döner), `CodeExchange` (kodun gizli değerini hub'da token'a çevirir); bir çağrının değerleri (`Ports`: saat, depo, açıcı) ve `AttachRequest` (kodun metni, `--trust-hub`, `--json`, stdin'den mi, terminal mi). `domain/` (paket `gecko-binding-domain`, I/O ve saat yok, `clippy.toml` ile): `code.rs` `gk1_` kodunun açılması (`open`: çözülür, hub adresi denetlenmiş origin olur; `AttachCode` alanları özel) ve hub adresi kuralı (`hub_origin`: yalnız `https://` ya da bu bilgisayarda `http://`; adresin URL olarak ayrıştırılması module'de, kural ayrıştırılmış parçalar üzerinde); `attach.rs` güven sorusu, cevabın okunması, sorunun sorulabileceği durum, seviye metni, `attach` sonucu; `detach.rs` `detach` sonucu; `token.rs` yenileme zamanı (bitişine 30 sn kala; saat değer olarak gelir), bitiş zamanı, hub'ın reddinin anlamı; `status.rs` durum adları, `ago`, düz ve `--json` `status` çıktısı (`StatusData`, `HubReach`, `BindingData` alanları özel, yapıcılarla kurulur). `module/` (paket `gecko-binding-module`): `features/attach.rs` (kodu açar, adres güveni, kodu `CodeExchange` port'uyla harcar, eski bağlamayı bitirir, token'ı yazar), `features/detach.rs`, `features/status.rs` (bağlantı süresi monoton zamanlayıcıyla burada); `code.rs` adresin `url` ile ayrıştırılması; `adapters/fs.rs` `Config`: `token.json`, `config.json` (`trustedHubs`) ve dosya izinleri (POSIX 0600/0700, Windows `icacls`), `TokenStore`'u uygular; `adapters/http.rs` `Auth` ve onu `CodeExchange` olarak saran `HttpExchange`: `POST /cli/attach` ile kodun harcanması ve `/oauth/token` ile yenileme (reddedilen yenileme bağlamanın bittiği demektir, token silinir). I/O yalnız `src/adapters/` altında. Modüller özel; ikili `attach` (`HttpExchange`'i kurup dilime veren sarmalayıcı), `detach`, `status`, `Config`, `Auth`, `Ports` ve `AttachRequest` adlarını crate kökünden alır.
- `crates/dev-hub/` (paket `gecko-dev-hub`): `gecko dev-hub` (K1). `hub.rs` kayıtlar ve kurallar (bağlama kodu: hub sırrın yalnız SHA-256 özetini tutar; kodun harcanması, adres başına yanlış deneme sınırı, token, bağlama ve onay seviyesi, sayfa yenilemesinde `unload`/`resume`, askıdaki bağlamaya giden `send`'in bekletilmesi, damga, hız sınırı, heartbeat); `server.rs` HTTP/1.1 ve `/hub` WebSocket yükseltmesi, bağlantı başına tek istek; `random.rs` sırlar ve base64url. `dev-hub` cargo özelliğinin arkasında: varsayılan derleme özelliği açar, sürüm derlemesi `--no-default-features` ile yapılır ve "this build has no dev hub" der; `sha2` yalnız bu crate'te olduğundan sürüm ikilisine girmez. Hub'ın testleri bu crate'te değil, `crates/testkit/tests/hub.rs`'te: testkit hub'a bağlı olduğundan hub'ın testkit'e dev-dependency'si paket grafiğinde döngü kurardı.
- `tests/signalr-hub/`: SignalR istemci testleri için en küçük ASP.NET Core hub'ı (CI `signalr` işi).
- Yayın: `.github/workflows/release.yml`. Akış: `Cargo.toml`'daki `version` ve `CHANGELOG.md`'deki `## <sürüm>` bölümü aynı PR'da yazılır. Merge'den sonra iş akışı elle çalıştırılır; bu deneme yalnız derler. Sonra `v<sürüm>` tag'i atılır. Tag altı hedefi `--no-default-features` ile derler, Release'i `SHA256SUMS` ve CHANGELOG bölümüyle açar, sonra `install.sh` / `install.ps1` ile yayımlanan sürümü kurup sürümünü denetler. Kurulum betikleri repo kökündedir; README onları `main` dalından indirir.
- Ortam değişkenleri ve test kancaları: [docs/test-hooks.md](docs/test-hooks.md).
- Node yalnız Prettier içindir: `package.json`'daki tek bağımlılık `prettier`; Markdown, YAML ve JSON dosyaları onunla biçimlenir.

Workspace üyeleri kök `Cargo.toml`'da açık bir listedir (`crates/*` deseni iç içe klasörlerde kırılır); paket adları klasör adından bağımsızdır.

## Mimari

Kararlar ve gerekçeleri: [docs/adr/0001-architecture.md](docs/adr/0001-architecture.md) (roller, port'lar, neden `session` bağlamı yok, neden `dev-hub` tek crate, rust-kit denetimine geçiş).

`crates/app/tests/architecture.rs` her crate'in rolünü ve kimin kime bağlanabileceğini denetler. Kontroller `cargo metadata` çıktısı, manifest ve kaynak metni üzerinde saf işlevlerdir; her birinin bozuk girdiyle kırmızı veren birim testi vardır. Yeni crate testteki `ROLES` listesine bir rolle girmeden test kırmızıdır.

| Rol            | Crate'ler (bugün)                                  | Bağlanabilir                                                                                     |
| -------------- | -------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `kernel`       | `gecko-kernel`                                     | yalnız izinli dış crate'ler (`serde`, `serde_json`)                                              |
| `protocol`     | `gecko-wire`, `gecko-hub-protocol`                 | `kernel`, izinli dış crate'ler                                                                   |
| `contract`     | `gecko-command-contract`, `gecko-binding-contract` | `kernel`, `protocol`                                                                             |
| `domain`       | `gecko-command-domain`, `gecko-binding-domain`     | kendi `contract`'ı, `kernel`, `protocol`; `clippy.toml` kopyası zorunlu                          |
| `module`       | `gecko-command-module`, `gecko-binding-module`     | kendi `domain`'i, `contract`, `kernel`, `protocol`, `adapter-lib`; I/O `src/adapters/**` altında |
| `adapter-lib`  | `gecko-hub-client`                                 | `kernel`, `protocol`                                                                             |
| `dev-tool`     | `gecko-dev-hub`                                    | workspace crate'i yok; ona yalnız ikili (özellik arkasında) ve `test-support` bağlanır           |
| `test-support` | `gecko-testkit`                                    | herkes; yalnız `dev-dependencies` içinde görünür                                                 |
| `app`          | `gecko`                                            | herkes (`test-support` yalnız dev)                                                               |
| `legacy`       | yok (B4'te boşaldı)                                | herkes; liste yalnız kısalır                                                                     |

Ayrıca: her manifest `[lints] workspace = true` taşır ve kök lint tablosu testtekiyle aynıdır; hiçbir crate kaynağı yol ile içeri almaz (bekleyen istisna yok); `--no-default-features` ile çözülen `gecko` grafiğinde `gecko-dev-hub`, `sha2` ve `gecko-testkit` yoktur.

Bu testin yanında repo sahipleri rust-kit'in denetimini (`cargo xtask arch`) reponun bir kopyası üzerinde, repo dışında koşar. O denetim bu repoya girmez ve bu testin yerini almaz; oradan gelen bulgu burada, herhangi bir inceleme yorumu gibi düzeltilir.

`deny.toml` (`cargo deny`, CI'da `deny` işi) lisans, güvenlik bildirimi ve kaynak denetimidir. Lisans izin listesinde ISC (`ring`, `untrusted`, `rustls-webpki`) ve CDLA-Permissive-2.0 (`webpki-root-certs`) bilinçli olarak var; yeni lisans PR'da gerekçelendirilir.

**Kapı dosyaları:** `crates/app/tests/architecture.rs`, bütün `clippy.toml` dosyaları, kök `Cargo.toml`'un `[workspace.lints.*]` bölümü ve `deny.toml`. Bunlara dokunan değişiklik ayrı bir PR'dır; gerekçesi PR açıklamasında yazar.

### Tarif: neyi nereye yazarsın

**Yeni bir komut** (`gecko <ad>`):

1. Bağlamı seç. Bağlama, token ya da hub adresiyle ilgiliyse `binding`; sekmeye giden satırla ilgiliyse `command`. İkisi de değilse yeni bağlam: `crates/contexts/<bağlam>/{contract,domain,module}`, kök `Cargo.toml`'da `members` ve `[workspace.dependencies]`, mimari testinin `ROLES` listesine üç satır (rol listesine ekleme kapı PR'ı sayılmaz), `domain`'e mevcut bir `clippy.toml`'un kopyası, `tests/exit_codes.rs`'in `SOURCE_ROOTS`'una üç `src` klasörü.
2. Kural (`domain/src/<ad>.rs`): karar, metin, sonuç biçimi. Saat ve kimlik değer olarak gelir; birim testi aynı dosyada.
3. Dilim (`module/src/features/<ad>.rs`): kuralı çağırır, dünyaya port'larla erişir (hub `HubConnection`, ekran `Reporter`, Ctrl+C `Interrupts`, saat `Clock`). Dosya, ağ ya da süreç gerekiyorsa `module/src/adapters/` altına.
4. Yeni bir port gerekiyorsa trait bağlamın `contract`'ına (yalnız o bağlam kullanıyorsa) ya da `gecko-kernel`'e (birden çok bağlam kullanıyorsa); uygulaması `module/src/adapters/`'ta ya da ikilide.
5. İkili (`crates/app`): `own.rs`'e ad ve kullanım metni; `main.rs`'de dağıtım; `src/<bağlam>.rs`'te argümanlar ve port'ların kurulumu. Kural ikiliye yazılmaz.
6. Yeni hata kodu: `gecko-kernel` `exit_code_of` tablosu ve `crates/app/tests/exit_codes.rs`.
7. Uçtan uca test `crates/app/tests/e2e.rs`'e, `gecko-testkit` ile.

**Hub taşıması** (gerçek hub, SignalR):

1. Taşıma `gecko-hub-client`'ta `HubConnection`'ı uygular (`signalr/`). Yeni bir sözleşme tipi ya da kapatma nedeni `gecko-hub-protocol`'e (`Closed` ve `failure.rs`'teki hata eşlemesi).
2. Taşıma seçimi `gecko-hub-protocol` `choose`; bağlantıyı açmak ikilinin `session.rs`'i. İki taşıma bir `enum` olur ve `HubConnection`'ı uygular; generic kalır, `dyn` değil. `HubOpener::Hub` ve `Session<H>` bu tipi taşır.
3. `domain` ve `module` crate'leri değişmez. Değişmesi gerekiyorsa sınır yanlış çizilmiştir; dur ve sor.
4. `tokio::net`, `reqwest` ve WebSocket yığını yalnız `gecko-hub-client`'ta ve module'lerin `adapters/` klasöründe; iç rollerde (`kernel`, `protocol`, `contract`, `domain`) `tokio` hiç yok.

## Kurallar

- `gecko` çağrıları sırayla yapılır. Aynı bağlamadaki paralel çağrılar `call.lock` (config dizininde, `token.json`'ın yanında) üzerinde sıraya girer; en çok 30 sn bekler, sonra `busy`. Kilidi işletim sistemi tutar, süreç bitince ya da çökünce bırakılır.
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

`deny.toml` ya da bağımlılık değiştiyse `cargo deny check` da.

Markdown, YAML ya da JSON değiştiyse `pnpm install && pnpm exec prettier --check .` da. İş ortasında yalnız değişen testler; PR öncesi bir kez tam paket.
