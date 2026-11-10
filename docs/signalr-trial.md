# SignalR denemesi (adım 4a)

Tarih: 2026-11-10. Spec: [rust-migration.md](rust-migration.md) §7.4. Sonuç: **elle yazılmış dar istemci.** `signalr-client` crate'i token'ı adreste taşıdığı için kullanılamaz.

## Soru

Gerçek Gecko hub'ı (`dolusoft/gecko-project-backend#1974`) SignalR olacak. Rust CLI'nın ona hazır bir crate ile mi, kendi yazdığımız dar bir istemciyle mi bağlanacağı denemeyle kararlaştırılacaktı. CLI'nın kullanımı dar: çağrı başına tek kısa bağlantı, JSON, WebSocket, akış yok, yeniden bağlanma yok.

## Düzenek

- `tests/signalr-hub/`: en küçük ASP.NET Core 10 hub'ı. #1974'teki davranışı taklit eder: token yalnız `Authorization` başlığından okunur, sorgu dizesindeki `access_token` 401 alır; `Status` ve `Send` `{ ok, value | error }` döner; sunucu `Reply` olayı yollar; `"4409 busy"` / `"4410 revoked"` metniyle `Close` gönderir; Azure tarzı `negotiate` yönlendirmesi (`url` + `accessToken`) verir; sonuç bekleyen sunucu çağrısı yapar. Ayarlar `TEST_HUB_KEEPALIVE_MS` ve `TEST_HUB_CLIENT_TIMEOUT_MS` ile.
- `spikes/signalr-crate/`: `signalr-client` 0.3.2 ile aynı hub'a bağlanır.
- `spikes/signalr-manual/`: `tokio-tungstenite` üstünde elle yazılmış istemci (`protocol.rs` çerçeve ve mesajlar, `connection.rs` bağlantı, `stream.rs` TCP/TLS). 9 birim, 12 entegrasyon testi.

İki deneme crate'i adım 4b-1'de silindi; elle istemci bugün `crates/gecko-hub-client/src/signalr/`'da (aşağıda "Sonraki adım"). Crate denemesinin kodu `eee7f30` commit'inde duruyor.

## Sonuç tablosu

Crate hub'a bağlanamadığı için (ilk satır) sütunundaki diğer satırlar crate'in kaynağından okundu (`communication/common.rs`, `communication/client_tokio.rs`, `execution/storage.rs`); kimlik, bağımlılık ve boyut satırları çalıştırılarak ölçüldü. Elle istemcinin her satırı testle ya da çalıştırarak doğrulandı.

| Sınanan                                           | `signalr-client` 0.3.2                                                                                | Elle istemci                                                                                                          |
| ------------------------------------------------- | ----------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Yükseltmede `Authorization` başlığı               | **Yok.** Token WebSocket adresine `access_token=` olarak eklenir; hub 401 verir, bağlanamaz           | Var; adreste token yok                                                                                                |
| `negotiate` + `?id=`                              | Var (başlıkla)                                                                                        | Var                                                                                                                   |
| `skipNegotiation`                                 | Yok, her zaman `negotiate`                                                                            | Var, varsayılan                                                                                                       |
| `negotiate` yönlendirmesi (`url` + `accessToken`) | Yok, alanlar okunmaz                                                                                  | Var, en çok 5 adım                                                                                                    |
| El sıkışma `error` alanı                          | Okunmaz; ilk çerçeve gelince başarılı sayılır                                                         | `Closed::Handshake(metin)`                                                                                            |
| El sıkışma cevabı ile ilk mesajlar aynı çerçevede | Bölünür                                                                                               | Bölünür; kayıt çerçeveler arasında yarım kalırsa ayırıcı beklenir                                                     |
| Tek mesajda çok kayıt                             | Var                                                                                                   | Var (3 `Reply` sırayla)                                                                                               |
| İstemci `Ping`'i                                  | **Gönderilmez**; sunucu ilk `Ping`'i görmediği için düşürmez, ama düzen sunucu ayarına bağlı          | Hiçbir şey gönderilmeyen her `keep_alive` aralığında (varsayılan 15 sn)                                               |
| Sunucu sessizliği                                 | İzlenmez                                                                                              | `server_timeout` (varsayılan 30 sn) → `Closed::ServerTimeout`                                                         |
| `Close` mesajı (`error`, `allowReconnect`)        | Gelir, `error` atılır                                                                                 | `"<kod> <ad>"` → `Busy`, `Revoked`, `Unauthorized`; diğerleri `Server(metin)`; `allowReconnect` okunur ve yok sayılır |
| `Completion` hatası                               | Var                                                                                                   | `InvokeError::Failed(metin)`, bağlantı sürer                                                                          |
| Sonuç bekleyen sunucu çağrısı                     | Hedefe kayıtlı geri çağırma varsa cevaplar; yoksa cevapsız kalır                                      | Hata `Completion`'ı ile cevaplanır, sunucu beklemez                                                                   |
| Tanınmayan mesaj tipleri (2, 8, 9)                | 8 tanınır, 9 çözümlenemez                                                                             | Yok sayılır                                                                                                           |
| Yeniden bağlanma                                  | Otomatik, varsayılan açık                                                                             | Yok; komut asla iki kez gitmez                                                                                        |
| TLS                                               | İki yığın: WebSocket'te `native-tls` (Windows'ta `schannel`), `negotiate`'te `ureq` üstünden `rustls` | Tek yığın: `rustls` + `ring` + `rustls-platform-verifier`, HTTP ve WebSocket aynı                                     |
| Ürün dışı bağımlılıklar                           | `test-log`, `console_log`, `console_error_panic_hook`, `tokio` `full`                                 | Yok                                                                                                                   |
| Bağımlılık (normal, tekil)                        | 122                                                                                                   | 107 (`reqwest`, `hyper`, `rustls` dahil; bunlar CLI'da zaten olacak)                                                  |
| Release ikili (strip, LTO, `opt-level = "z"`)     | 2,19 MB                                                                                               | 2,42 MB                                                                                                               |

## Ölçümler (Windows 11, yerel hub, debug ikili)

- Bağlantı (TCP + yükseltme + el sıkışma): 2–4 ms; `negotiate` ile 4 ms. İlk süreçte 14–43 ms (soğuk).
- `Status` çağrısı: 0,25–1 ms.
- Kısa `keep_alive` (1 sn) ile 2 sn'lik sunucu süresinde 8 sn boşta: bağlantı yaşadı. `keep_alive` 3 sn'de sunucu düşürdü (`Close`, "Connection closed with an error.").
- Sunucu `Ping`'i 500 ms'de, istemci sınırı 200 ms: `ServerTimeout`.
- Varsayılan ayarlarla (sunucu 15 sn / 30 sn, istemci 15 sn / 30 sn) 130 sn boşta: bağlantı yaşadı; ardından `Status` 13 ms, `Send` + `Reply` 3 ms.
- TLS: `https://example.com` yükseltmesi 404, `negotiate` 405 (el sıkışma geçti); `https://expired.badssl.com` "invalid peer certificate: Expired".

## Bulunan bir ASP.NET Core ayrıntısı

Sunucu istemci zaman aşımını (`ClientTimeoutInterval`) istemciden **ilk `Ping` geldikten sonra** başlatır. `Ping` göndermeyen bir istemci boşta düşmez, gönderen istemci ise aralığa uymak zorundadır. Deneme bu yüzden iki yönlü yapıldı: aralığın altında yaşar, üstünde düşer.

## Karar

Rust CLI SignalR'a **elle yazılmış dar istemciyle** bağlanır. Gerekçe:

1. `signalr-client` token'ı adrese koyar. Bu, AGENTS.md'deki "token yalnız `Authorization` başlığında" kuralını ve #1974'ün 4403 kuralını çiğner; hub bağlantıyı reddeder. Crate'i yamamak, kalan eksikleri (`skipNegotiation`, `Close` nedeni, `Ping`, el sıkışma hatası, iki TLS yığını) da üstlenmek demek.
2. Elle istemci küçük: protokol, bağlantı ve akış birlikte yaklaşık 750 satır (`rustfmt` sonrası), testlerle 1100. Desteklediği alt küme açıkça yazılı.
3. TLS ve vekil kuralı HTTP ile ortak; kurum içi sertifika otoritesi işletim sistemi deposundan gelir.

Desteklenmeyenler (bilerek): MessagePack, long polling, server-sent events, stateful reconnect, akış, otomatik yeniden bağlanma.

## Sonraki adım

**Yapıldı (adım 4b-1):** istemci `crates/gecko-hub-client`'e taşındı (`src/signalr/` protokol ve bağlantı, `src/net.rs` TCP/TLS ve tünel, `src/proxy.rs` vekil kuralı); `spikes/` silindi, CI'ın `signalr` işi `cargo test -p gecko-hub-client` koşuyor. Denemeden kalan üç iş de bu adımda bitti:

- Vekil: `https_proxy`/`http_proxy`/`all_proxy` ve `no_proxy` (büyük harfli adlar da) tek kuraldır; `negotiate` (reqwest'in kendi vekil araması kapalı) ve WebSocket aynı kurala uyar. WebSocket için `CONNECT` tüneli, adresteki `user:password` ile `Proxy-Authorization: Basic`. Yalnız `http://` vekil; `https://` ve SOCKS vekil açık hata verir, `no_proxy`'de CIDR yok. Test: yerel bir `CONNECT` vekili üstünden bağlantı, `no_proxy` ile doğrudan bağlantı, ulaşılamayan vekil.
- Hata metnindeki HTTP gövdesi tek satıra indirilir ve 200 karakterde kesilir.
- `invoke` başına cevap süresi (`Options::invoke_timeout`, varsayılan 10 sn, Node'daki RPC ile aynı). Süre dolunca `InvokeError::TimedOut`; bağlantı açık kalır, geç gelen cevap atılır. Sonucun bilinmediğine çağıran karar verir.

`Transport` trait'inin SignalR uygulaması adım 7'de, #1974 yöntem adlarının SignalR yazımını verince gelir.
