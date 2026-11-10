# Ortam değişkenleri ve test kancaları

Bu liste sözleşmenin parçasıdır ([rust-migration.md](rust-migration.md) §7.1): sözleşme testleri CLI'yı ve geliştirme hub'ını bu değişkenlerle sürer. Rust ikilisi hepsini aynı adla ve aynı anlamla okur. Yeni bir kanca önce buraya yazılır.

Değerler milisaniye (`_MS`) ya da `1` (açık) olarak verilir. Değişken yoksa varsayılan geçerlidir.

## Kullanıcı ayarları

Testler de kullanır, ama kullanıcıya açıktır; README'de anlatılır.

| Değişken           | Anlamı                                                                                        | Varsayılan                                                                                        |
| ------------------ | --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `GECKO_CONFIG_DIR` | Token ve ayar dosyalarının dizini                                                             | Windows `%APPDATA%\gecko-cli`, diğerleri `$XDG_CONFIG_HOME/gecko-cli` ya da `~/.config/gecko-cli` |
| `GECKO_BROWSER`    | `none`: tarayıcı açılmaz, adres yalnız yazılır (SSH ve masaüstü olmayan oturumda da böyledir) | Masaüstü varsa açılır                                                                             |
| `GECKO_ENV_<AD>`   | `--env <ad>` için adres takma adı; `<AD>` büyük harfle (`GECKO_ENV_STAGE=stage.ornek.com`)    | Yok                                                                                               |

Vekil için genel değişkenler okunur (yalnız Rust ikilisi; [signalr-trial.md](signalr-trial.md)): `https_proxy`, `http_proxy`, `all_proxy`, `no_proxy`, ve büyük harfli adları; ikisi de varsa küçük harfli geçerlidir. HTTP ve WebSocket aynı kurala uyar. Yalnız `http://` vekil desteklenir.

## CLI test kancaları

| Değişken                     | Anlamı                                                                                                            | Varsayılan        |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------- | ----------------- |
| `GECKO_TEST_INTERRUPT_STDIN` | `1`: stdin'e yazılan her `x` bir Ctrl+C sayılır. Windows alt sürece SIGINT iletemediği için testler bunu kullanır | Kapalı            |
| `GECKO_TEST_TTY`             | `1`: stdout ve stderr terminal sayılır (renk, ilerleme satırı, onay sorusu)                                       | Gerçek TTY durumu |
| `GECKO_DELIVERY_MARGIN_MS`   | Teslim penceresine eklenen pay; `received` bu süre de dolunca gelmezse okuma kod 4, değiştiren istek kod 5        | 2000              |
| `GECKO_CANCEL_WAIT_MS`       | Ctrl+C'den sonra `cancel` cevabının beklendiği süre                                                               | 3000              |
| `GECKO_CAPABILITIES_WAIT_MS` | Bağlamadan sonra yetenek listesinin beklendiği süre                                                               | 5000              |
| `GECKO_STATUS_WAIT_MS`       | `gecko status`'ta sekmeye ulaşma süresi                                                                           | 2000              |
| `GECKO_TRANSPORT`            | `dev`: adres localhost olmasa da geliştirme taşıması seçilir                                                      | Adrese göre       |

## Geliştirme hub'ı kancaları

| Değişken                | Anlamı                                                                            | Varsayılan                     |
| ----------------------- | --------------------------------------------------------------------------------- | ------------------------------ |
| `GECKO_DEV_HUB_RATE`    | Hız sınırı: yetki başına saniyede ortalama istek                                  | 5                              |
| `GECKO_DEV_HUB_BURST`   | Hız sınırının patlama payı                                                        | 20 ya da oran, hangisi büyükse |
| `GECKO_DEV_HUB_USERS`   | Kullanıcı listesi, JSON dizi (`[{ "id", "name" }]`)                               | Tek test kullanıcısı (`u1`)    |
| `GECKO_DEV_HUB_TIMINGS` | Süreler, `DevHubTimings` biçiminde JSON (testler §5 ve §8 pencerelerini kısaltır) | Spec'teki süreler              |
| `GECKO_DEV_HUB_STDIN`   | `1`: stdin kapanınca hub durur (testler süreç ağacını böyle kapatır)              | Kapalı                         |

## SignalR test hub'ı

`tests/signalr-hub/` yalnız Rust istemcisinin testleri içindir; ürünün parçası değildir ([signalr-trial.md](signalr-trial.md)).

| Değişken                          | Anlamı                                                                | Varsayılan |
| --------------------------------- | --------------------------------------------------------------------- | ---------- |
| `TEST_HUB_KEEPALIVE_MS`           | Hub'ın `Ping` aralığı (`KeepAliveInterval`)                           | 15000      |
| `TEST_HUB_CLIENT_TIMEOUT_MS`      | Hub'ın istemciyi düşürdüğü sessizlik süresi (`ClientTimeoutInterval`) | 30000      |
| `GECKO_SIGNALR_TEST_HUB`          | İstemci testlerinin bağlandığı hub adresi; yoksa testler atlanır      | Yok        |
| `GECKO_SIGNALR_TEST_HUB_REQUIRED` | `1`: hub adresi yoksa testler düşer (CI'ın `signalr` işi koyar)       | Kapalı     |
