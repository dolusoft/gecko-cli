# Tarayıcı tarafı: frontendx hub'a ne der

Bu belge, Gecko sekmesinin (frontendx) masaüstü CLI için hub'la nasıl konuşacağını anlatır. Kaynak tasarım [frontend-v2-design `design/desktop-cli.md`](https://github.com/dolusoft/frontend-v2-design/blob/main/design/desktop-cli.md) (3. taslak); bölüm numaraları oradandır. Burada yazılanlar geliştirme hub'ının (`gecko dev-hub`) bugün uyguladığı sözleşmedir. Gerçek hub (SignalR, [gecko-project-backend#1974](https://github.com/dolusoft/gecko-project-backend/issues/1974)) aynı yöntem ve olayları taşıyacak; yalnız çerçeve ve yöntem adlarının büyük harf biçimi değişebilir.

## Kullanıcı ne görür

1. Terminalde `gecko attach` yazılır. CLI bir kod basar ve tarayıcıda `https://<ortam>/cli-pair#<kod>` adresini açar.
2. Sayfa "Bu sekme CLI'ya bağlansın mı?" diye sorar; yanında yetkinin oluşturulduğu saat ve ortam görünür. CLI'nın beyan ettiği bilgisayar adı **gösterilmez**.
3. Kullanıcı onaylar; sayfa normal Gecko sayfasına geçer. Terminalde "CLI bağlı" göstergesi ve "bağlantıyı kes" düğmesi görünür.
4. CLI'dan gelen satır terminal geçmişine "CLI" işaretiyle düşer. Değiştiren komut onay penceresi açar.

Açık bir sekmede `cli.attach <kod>` yazmak da aynı bağlamayı kurar (telefondaki sekme, SSH'den gelen kod).

## frontendx'in yapacakları

| İş                             | Nerede                      | Not                                                                                                                                                                                    |
| ------------------------------ | --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/cli-pair` sayfası            | yeni rota                   | Kodu `location.hash`'ten okur ve hemen siler (`history.replaceState`). Oturum kapalıysa kodu girişten önce `sessionStorage`'a koyar, dönüşte okur ve siler. Kod yoksa elle yazma alanı |
| `cli.attach <kod>`             | terminal komutu             | `sensitive` değil; kod tek kullanımlık ve onay ister                                                                                                                                   |
| Bağlama sorusu                 | `/cli-pair` ve `cli.attach` | `pairInfo` ile sunucu bilgisini alır, kullanıcı onaylarsa `pair`                                                                                                                       |
| Sayfa kimliği                  | kabuk                       | Her sayfa yüklemesinde yeni `pageId` (`crypto.randomUUID()`); yeniden bağlanmada `hello` ile bildirilir                                                                                |
| Yaşam döngüsü                  | kabuk                       | `pagehide`/`beforeunload` → `unload`; `freeze`/`resume` ve geri/ileri önbelleğinden dönüş → yeniden `hello`; çıkış → `logout`                                                          |
| Uzak giriş                     | kabuk                       | Bağlama kurulunca `console.attachRemote(binding)`; bağlantı düşünce `suspendRemote()`; `gone` gelince `detachRemote()`                                                                 |
| "Veri gerekli" talebi          | görev store'u önce          | CLI isteği sürerken akış açık kalır, en çok 5 sn taze veri beklenir (§5.4)                                                                                                             |
| `confirm` alanı                | değiştiren komutlar         | `CommandSpec.confirm = { field: 'yes' }` ve `plan(input, signal)`                                                                                                                      |
| Onay penceresi, CLI göstergesi | command-console 1.6         | `.cc-approval`, `.cc-remote`, `.cc-pairing` hazır; kabuk yalnız olayları bağlar                                                                                                        |

## Bağlantı

- Adres: `ws(s)://<hub>/hub`. Tarayıcı başlık yollayamadığı için oturum token'ı sorgu dizesinde gider: `?access_token=<oturum>`. Geliştirme hub'ında oturum `dev:<kullanıcı>` biçimindedir (`dev:u1`).
- CLI token'ı (`cli_…`) sorgu dizesinde gelirse hub bağlantıyı `4403` ile kapatır; CLI token'ı yalnız `Authorization` başlığıyla kabul edilir.
- Hub bağlantı açılınca `ready` olayını yollar. Sekme ilk iş `hello` çağırır.
- Hub her tarayıcı bağlantısına 4 sn'de bir WebSocket `ping` yollar. Arka arkaya iki `ping` cevapsız kalırsa (ağ kesildikten 8–12 sn sonra) bağlantı kopmuş sayılır ve bağlama `suspended` olur; bağlantı kapanmasa da CLI isteği hemen `tab-unreachable` alır. `pong` ya da herhangi bir çerçeve yeniden gelirse bağlama `active`'e döner; 60 sn askı kuralı aynıdır. Tarayıcı `pong`'u kendiliğinden yollar, sekmenin bir şey yapması gerekmez.

## Çerçeve (geliştirme taşıması)

Tek WebSocket üstünde JSON. Gerçek hub'da SignalR çağrısı ve olayı olacak.

```json
{"t":"invoke","id":1,"method":"hello","args":{"pageId":"p-123"}}
{"t":"result","id":1,"ok":true,"value":{"binding":null}}
{"t":"result","id":2,"ok":false,"error":{"code":"invalid-code","message":"…"}}
{"t":"event","name":"cli-message","payload":{…}}
```

`@dolusoft/gecko-cli/tab-client` paketindeki `connectTab()` bunu sarar; frontendx'in geliştirme ortamı ve testleri doğrudan kullanabilir. `createTabGate()` aynı istemciden command-console-core'un `ConsoleGate`'ini kurar.

## Sekmenin yöntemleri

| Yöntem      | Argüman        | Dönüş                                                     | Hatalar                                                                    |
| ----------- | -------------- | --------------------------------------------------------- | -------------------------------------------------------------------------- |
| `hello`     | `{ pageId }`   | `{ binding: { bindingId, grantId, approvedAt } \| null }` | `invalid`                                                                  |
| `pairInfo`  | `{ code }`     | `{ grantCreatedAt, env }`                                 | `invalid-code`, `other-account`, `no-terminal-access`, `too-many-attempts` |
| `pair`      | `{ code }`     | `{ bindingId, grantId, approvedAt }`                      | `pairInfo`'nunkiler; önce `hello` gerekir                                  |
| `sendToCli` | `{ reply }`    | `{ delivered }`                                           | `not-bound`                                                                |
| `detach`    | —              | `{ ok }`                                                  | —                                                                          |
| `unload`    | —              | `{ ok }`                                                  | —                                                                          |
| `logout`    | —              | `{ ok }`                                                  | —                                                                          |
| `broadcast` | `{ envelope }` | `{ delivered }`                                           | —                                                                          |

- **`hello`** her bağlantıda ve sayfa kimliği değişince çağrılır. Askıdaki (`suspended`) bir bağlama 60 sn içinde aynı `pageId` ile gelirse `active`'e döner ve `binding` dolu gelir. Sekme o zaman command-console'da bağlamayı korur (`suspendRemote()` bağlamayı düşürmez); çekirdeğin bağlaması yoksa `attachRemote(binding)` yeniden çağrılır.
- **`pairInfo`** kodu tüketmez. Kod başka hesaba aitse hub CLI'ya da haber verir; CLI "tarayıcıda başka hesap açık" der ve beklemeyi sürdürür. Terminal erişimi yoksa kod tüketilmez, CLI kod 3 ile çıkar.
- **`pair`** kodu atomik olarak tüketir. Bu sayfanın eski bağlaması ve aynı yetkinin başka sekmedeki bağlaması `gone`, `replaced` olur.
- **`sendToCli`** yalnız bağlamanın tarayıcı bağlantısından kabul edilir (bugünkü `Send` bütün gruba dağıtıyor; bu ayrı bir yöntemdir). Hub zarfın yalnız `kind` alanına bakar: `capabilities` cevabını bir sonraki CLI bağlantısı için saklar, çünkü her `gecko` çağrısı yeni bir bağlantıdır.
- **`detach`**: terminaldeki "bağlantıyı kes" düğmesi. Bağlama `gone`, `detached`.
- **`unload`**: sayfa kapanıyor ya da yenileniyor. Bağlama `gone`, `closed`.
- **`broadcast`**: bugünkü tarayıcılar arası yayın. CLI bağlantıları bu yayına hiç katılmaz.

## Sekmeye gelen olaylar

| Olay          | Yük                                    | Sekme ne yapar                                                                                                          |
| ------------- | -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `cli-message` | damgalı `cli-request` ya da `cancel`   | Ham değeri `ConsoleGate.subscribe` dinleyicisine verir; çekirdek `parseCliMessage` ile okur, damgayı ve süreyi denetler |
| `binding`     | `{ bindingId, state: 'gone', reason }` | `detachRemote()`; terminal göstergesini kaldırır                                                                        |
| `cli-busy`    | `{ grantId, at }`                      | Terminalde "başka bir CLI çağrısı denendi" gösterir (§3)                                                                |
| `broadcast`   | zarf                                   | Bugünkü yayın yolu                                                                                                      |

Damga hub'ındır: `origin: { kind: 'cli', grantId, bindingId }`, `stampedAt`, `expiresAt = stampedAt + 30 sn`. İstemcinin yazdığı damga alanları hub'da silinip yeniden yazılır. `actor: 'cli'` yalnız geçmiş etiketidir; izin damgaya ve bağlamaya bakar.

## Sekmeden CLI'ya giden cevaplar

Hepsini command-console-core 1.6 üretir; kabuk yalnız `RemoteBinding.send` ile `sendToCli`'ya iletir:

`capabilities` (bağlama kurulunca bir kez ve komutlar değişince) → `received { hidden, dataAgeMs }` → `awaiting-approval` (yalnız değiştiren komutta) → en çok üç `chunk` → `finished`. İptalde `cancelled-before-start` ya da `cancel-requested`.

`RemoteBinding.status()` kabuğun bildirdiği görünürlüğü (`document.visibilityState === 'hidden'`) ve verinin yaşını verir; CLI bunu "sekme arka planda, liste 42 sn eski" diye basar.

## `gecko help`: sekmenin yardımı

Kullanıcı terminalde `gecko help`, `gecko help tasks` ya da `gecko help tasks.list` yazar ve bağlı sekmenin komutlarını, kendi iznine göre süzülmüş ve sayfalı olarak görür. Sekmenin kendi `help` komutu CLI'ya açık değildir (`help` satırı yollamak `not-allowed` döner). Bu yüzden CLI satır yerine ayrı bir istek yollar; sekme cevabı command-console-core'un ajan `help` aracından (`agentTools(console, …)[1].execute`) alır.

CLI → hub (`send`), hub bunu `cli-request` gibi damgalar, hız sınırına sayar ve `cli-message` olayıyla sekmeye iletir:

```json
{
  "v": 1,
  "kind": "help-request",
  "id": "<uuid>",
  "input": { "prefix": "tasks", "cursor": "30", "limit": 30 },
  "origin": { "kind": "cli", "grantId": "…", "bindingId": "…" },
  "stampedAt": 1762480000000,
  "expiresAt": 1762480030000
}
```

`input` ajan `help` aracının girdisidir; alanların hepsi isteğe bağlıdır:

| Alan      | CLI'da                                                               | Anlamı                              |
| --------- | -------------------------------------------------------------------- | ----------------------------------- |
| `command` | `gecko help tasks.list` (ad ya da takma ad yetenek listesinde varsa) | o komutun kullanımı ve girdi şeması |
| `prefix`  | `gecko help tasks` (komut adı değilse)                               | yalnız o ad alanının komutları      |
| `cursor`  | `--cursor <n>`                                                       | önceki sayfanın `nextCursor` değeri |
| `limit`   | `--limit <n>`                                                        | sayfa boyu (aracın sınırı geçerli)  |

`gecko <komut> --help` de `gecko help <komut>` ile aynıdır.

Sekme → CLI (`sendToCli`), hub zarfa bakmadan iletir:

```json
{ "v": 1, "kind": "help-result", "id": "<aynı uuid>", "result": { "namespaces": […], "commands": […], "nextCursor": "30" } }
```

`result` aracın çıktısıdır, olduğu gibi: sayfa `{ namespaces, commands, nextCursor? }`, tek komut `{ name, summary, usage, danger, confirm?, input }` ya da hata `{ status: "error", code, message? }`. Terminal erişimi yoksa sekme `{ status: "error", code: "no-terminal-access" }` döner (CLI kod 3 ile çıkar).

Sekmenin kuralları:

- `origin.bindingId` sekmenin şu anki bağlamasından farklıysa ya da `expiresAt` geçtiyse cevap verilmez.
- `help-request` `ConsoleGate`'e verilse de zarar yok: çekirdek yalnız `cli-request` ve `cancel` okur, bu türü yok sayar.
- Cevap gelmezse CLI damga penceresi (+2 sn) dolunca `not-delivered` (kod 4) ile "Gecko sayfasını yenileyin" der.

`@dolusoft/gecko-cli/tab-client` paketindeki `serveHelp(tab, { help, bindingId, allowed?, now? })` bunu hazır yapar; `readHelpRequest(raw)` yalnız okumayı verir.

## Bağlamanın hâlleri

| Hâl         | Ne zaman                                                               | CLI'ya etkisi                                                 |
| ----------- | ---------------------------------------------------------------------- | ------------------------------------------------------------- |
| `active`    | sekmenin bağlantısı açık                                               | istek iletilir                                                |
| `suspended` | sekmenin bağlantısı düştü ya da `ping`'e cevap yok, 60 sn askı         | istek hemen `tab-unreachable` (kod 4)                         |
| `gone`      | askı doldu, `unload`, `detach`, ikinci bağlama, `logout`, yetki iptali | `gone` + `reason` (kod 4); CLI kendiliğinden yeni sekme açmaz |

`reason` değerleri: `replaced`, `closed`, `detached`, `logged-out`, `revoked`, `suspended-timeout`.

## Kimlik ve giriş sayfası

- CLI girişi RFC 8628 cihaz akışıdır. Onay sayfası `/cli-login#<user_code>` adresidir; kalıbı `/cli-pair` ile aynı (kod `#` sonrasında, okunup silinir).
- Sayfa kodu `POST /oauth/device/verify` ile onaylar ya da reddeder: `{ user_code, approve }`, `Authorization: Bearer <oturum>`.
- Profilde CLI yetkileri: `GET /cli/grants` listeler, `POST /cli/grants/<grantId>/revoke` iptal eder. İptal hub'daki CLI bağlantısını kapatır (`4410`), bağlamaları `gone`, `revoked` yapar.

Geliştirme hub'ındaki `/cli-login` sayfası frontendx gelene kadar sahte kullanıcıyla onay verir; gerçek sayfa frontendx'tedir.

## Yerel geliştirme

```sh
gecko dev-hub --port 4400            # hub, yalnız 127.0.0.1
gecko login --env 127.0.0.1:4400     # /cli-login sayfasında onay
gecko attach                          # frontendx'in /cli-pair sayfası
```

frontendx geliştirme sunucusu hub'a `ws://127.0.0.1:4400/hub?access_token=dev:u1` ile bağlanır. Bağlama ve giriş adreslerinin frontendx'e gitmesi için hub `--web` ile başlatılır: `gecko dev-hub --web http://localhost:5173` (kütüphane olarak `startDevHub({ webOrigin: 'http://localhost:5173' })`).
