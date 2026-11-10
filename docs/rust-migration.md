# gecko CLI: Rust'a geçiş ve command-console'dan yalıtım

Taslak 2, onaylandı · 2026-11-10 Salı · Karar sahibi: Zahid · İnceleme: Astra (16 bulgu), Fable (36 bulgu) → taslak 1'e

Üst belge: frontend-v2-design `design/desktop-cli.md` (3. taslak) ve `design/desktop-cli-ux.md`. Bu belge onların aşağıdaki maddelerini **değiştirir**; geçişle birlikte üst belge ayrı PR'la güncellenir (F35): §4.1 (bilinmeyen komutun yolu), §6.1 (biçimlendirmenin sahibi), §8 (saklama gerekçesi `npx`), §10.1 (`quoteArgument` CLI'da), §10.5 (npm paketi, Node 22+, `command-protocol` bağımlılığı), §12 (`sensitive` satırı), §11 skill (kod 5 metni).

Bugünkü davranıştan sapan her madde **[FARK]** ile işaretli. İnceleme bulgularına atıf: `A<n>` Astra, `F<n>` Fable.

## 1. Amaç

1. **Rust'a tam geçiş.** `gecko` ikilisi %100 Rust; TypeScript ürün kodu silinir; Node gerekmez. Dağıtım GitHub Releases ve kurulum betiği (`irm | iex`, `curl | sh`). npm'de yayın yok.
2. **Yalıtım.** CLI ile command-console birbirinden bağımsız sürüm çıkarır.

**Yalıtım ölçütü.** command-console şunlardan birini yaptığında CLI'da tek satır değişmez, yeni CLI sürümü gerekmez:

- komut eklemek, silmek, yeniden adlandırmak, alias eklemek;
- bir komutun alanlarını, yardımını, önizleme davranışını değiştirmek;
- tokenizer'ı ya da tırnak kuralını değiştirmek (§5.3'teki değişmezlere dokunmadan);
- yeni bir hata kodu eklemek.

CLI'nın da sekmeye dokunmadan değiştirebildikleri: çıktı biçimi, taşıma, kendi komutları, kurulum.

## 2. Bugünkü bağlar

Kod: `gecko-cli@1ca54ae`, `command-console@2d514f6` (1.6.0), `frontendx@afcf2ba`.

| #   | Bağ                                                                     | Nerede                                    | Gideriliş                    |
| --- | ----------------------------------------------------------------------- | ----------------------------------------- | ---------------------------- |
| B1  | `quoteArgument()` ile argv'den satır                                    | `args.ts:160-174`, `suggest.ts`, `run.ts` | §5.1 `argv`                  |
| B2  | `tokenize()` ile satırı bölme                                           | `main.ts:523-596`, `run.ts:108`           | §5.1, §5.3                   |
| B3  | Komut adı yazım önerisi (Levenshtein)                                   | `suggest.ts`                              | §5.4, sekme                  |
| B4  | Konumsal düzeltme (`input` şemasını okur)                               | `suggest.ts:83`, `main.ts:517`            | §5.4, sekme                  |
| B5  | `confirm` alanı tespiti                                                 | `run.ts:143-145`                          | §5.2 `received.change`       |
| B6  | Yardımın girdisi ve çıktısı                                             | `help.ts:23-109`, `main.ts:446-461`       | §5.5                         |
| B7  | Hata kodu → çıkış kodu tablosu                                          | `output.ts:47-78`                         | §5.6                         |
| B8  | Zarf tipleri, `joinChunks`                                              | `run.ts`, `session.ts`                    | Kalır; §4 fixture            |
| B9  | `help-request` iki kopya (gecko-cli `wire.ts`, frontendx `cliHelp.ts`)  |                                           | §5.5, çekirdeğe              |
| B10 | Hub çerçevesi frontendx'te elle kopya                                   | `devHubTransport.ts`                      | §4 `fixtures/hub`            |
| B11 | Önizlemenin "çalıştırmak için" satırı CLI'da kuruluyor (A6, F8)         | `run.ts:305-309`                          | §5.4 `suggest` önizlemede de |
| B12 | Sayfa devamı satırı (`nextCursor`, `--cursor`) (A6, F9)                 | `output.ts:209-221`                       | §5.4 `next`                  |
| B13 | `nav.where` ve `title`/`path` alanları (A6, F11)                        | `status.ts:30-46`, `main.ts:283-295`      | §5.9 `where`                 |
| B14 | `tasks.` öneki (bayatlık), `dataview.` öneki (görünüm satırı) (A6, F11) | `run.ts:342`, `main.ts:300, 498-503`      | §5.2 `received` alanları     |
| B15 | Yanlış yerdeki seçenek tespiti (`--json` komuttan sonra) (A7)           | `args.ts:121-140`                         | §5.1, dar kural              |
| B16 | `formatData` `input` şemasını yardım diye tanıyor (A6, F12)             | `output.ts:223-342`                       | §5.5 ile silinir             |
| B17 | `outcome.line` ile yeni `suggest` aynı anlam (F13)                      | `remote.ts:123`, `run.ts:284`             | §5.4 tek alan                |
| B18 | WebSocket kapanış koduna dayalı durum (4409, 4410) (F14)                | `wire.ts`, `run.ts:403`, `session.ts:216` | §7.2 anlamsal neden          |

Bağ olmayanlar: genel tablo yazıcı ve sezgileri (süre alanları, `columns`+`rows`, `title`/`name`/`value`; bozulunca yalnız görünüm kötüleşir), renk, `\uXXXX` kaçışı, bekleme göstergesi, `--out`, hız sınırında yeniden deneme. CLI'nın kendi adları (`cli.attach <kod>` ipucu, `/cli-pair` yolu) hub sözleşmesine yazılır (§4).

## 3. İlke ve güven sınırı

> **CLI satırı anlamaz.** Komutun alanlarını, söz dilimini, yardımını, önerisini ve önizlemesini sekme bilir. CLI argv'yi taşır, cevabı genel kurallarla basar, çıkış kodunu sekmenin beyanından ve kendi gözleminden üretir.

**CLI'da kalan, sabit ve küçük dil bilgisi:**

1. CLI seçenekleri ve kendi komutları (komut adından önce).
2. Komut adının nerede bittiği (§5.3).
3. Komut adının bilinip bilinmediği: yetenek listesindeki adlarla saf veri karşılaştırması (§5.2).
4. Hassas ad süzgeci (§5.3).

Bunlar değişirse sözleşme değişikliğidir (§6), tokenizer değişikliği değil.

**Güven sınırı (A9, F18).** Sekme komutu çalıştıran taraftır; cevaplarına bugün de güveniliyor (`busy` diyerek kod 4 üretebilir). Tehdit modelinde kötü niyetli sekme yok, **hatalı** sekme var. CLI'nın ajanlara verdiği iki söz ("4 = çalışmadı, yeniden denemek güvenli", "hassas satır makineden çıkmaz") sekmenin hatasına karşı CLI'nın kendi gözlemiyle korunur (§5.6, §5.3).

## 4. Sözleşmenin evi ve biçimi

- **CLI ↔ sekme:** command-protocol `remote` bölümü; `help-request`/`help-result` dahil (B9).
- **CLI ↔ hub, sekme ↔ hub:** gecko-cli `docs/hub-protocol.md` + `fixtures/hub/`. frontendx bunlara karşı test koşar (B10).
- **Normatif tanım (A11).** Her zarf için alan tablosu: tip, zorunlu mu, varsayılan, bilinmeyen değerde okuyucu davranışı. Fixture'lar tanımın örneğidir, yerine geçmez.
- **Fixture sınıfları (A11, F24):**
  - `valid`: kabul edilir, beklenen okuma sonucuyla;
  - `invalid`: reddedilir, beklenen hatayla;
  - `future`: gelecekten alan, enum değeri, `kind`; beklenen "kabul et, yok say" ya da tanımlı varsayılan;
  - `sequence`: mesaj dizisi ve beklenen sonuç (kayıp `received`, yinelenen parça, iptal yarışı, kopma). Beklenen sonuç çıkış kodu ve JSON'dur.
- **Yayın.** Fixture'lar command-console GitHub Release ekinde sürümlü arşiv (`remote-fixtures-<sürüm>.tar.gz`). command-console repo public (MIT); indirme token istemez.

## 5. Değişiklikler

### 5.1 İstek: argv (B1, B2, B15)

```json
{"v":1,"kind":"cli-request","id":"…","argv":["tasks.stop","--id","8812"]}
{"v":1,"kind":"cli-request","id":"…","line":"tasks.stop --id 8812"}
```

- Komut adı ve sonrası → `argv`. `--line "<satır>"`, `--line -` → `line`. İkisi birden ya da hiçbiri → sekme `invalid-args` döner (§5.7 kural 1).
- **Sekme `argv`'yi satıra çevirmeden işler** (F17): `lookup.find(argv[0])` + `parseOptions(argv.slice(1))`. Satır yalnız geçmiş görünümü için kurulur (`formatCommandLine`'ın kanonik hâli, bugünkü gibi).
- **Sınırlar (A12, F17).** `argv`: en çok 256 öge, öge başına en çok 4000 kod noktası, toplam en çok 4000 kod noktası. `line`: en çok 4000 kod noktası. Bütün zarf: 16 KB (UTF-8, damga payı 512 bayt düşülmüş). CLI bunları **yollamadan önce** denetler; aşımda kod 2 `too-large`.
- **Yanlış yerdeki seçenek (B15).** Kural daralır: komut adından **hemen sonra** gelen ve değer almayan `--json`, `--out`, `--line` için CLI kod 2 "CLI seçenekleri komuttan önce gelir" der. Başka bir konumdakiler satıra aynen geçer. **[FARK]** Bugün argümanın değer olup olmadığına da bakılıyor (`args.ts:121-140`). Gerekçe: komutun kendi alanlarını bilmeden yapılabilen tek denetim bu.

### 5.2 Ön süzgeç ve `received` (B5, B14)

CLI, isteği yollamadan önce:

1. Komut adını bulur (§5.3). Bulunamazsa kod 2.
2. Adı hassas listede arar (§5.3). Bulursa kod 3; hiçbir şey gönderilmez.
3. **Adı yetenek listesinin adları, alias'ları ve `help` kümesinde arar** (A4, F4). Ad yoksa **argümanlar gönderilmez**: CLI yalnız adla bir `help-request` yollar (§5.5) ve sekmenin `unknown-command` cevabını, `suggest` ile birlikte basar; kod 2. Liste eskiyse (§5.8) CLI listeyi bir kez tazeleyip yeniden dener.

Böylece üst belgenin "listede olmayan komut sekmeye hiç gitmez" ve "hassas girdi ağa çıkmaz" vaatleri korunur; yazım önerisi de sekmede kalır. Üyelik denetimi satır bilgisi değil, veri karşılaştırmasıdır: komut eklemek CLI'ya dokunmaz.

`received` zarfı yeni alanlar taşır; hepsi sekmenin beyanıdır:

| Alan                        | Anlamı                                                     | Yerine geçtiği        |
| --------------------------- | ---------------------------------------------------------- | --------------------- |
| `change: boolean`           | Bu istek veri değiştiren yoldan çalışacak (önizleme değil) | B5                    |
| `dataAgeMs: number \| null` | Var; bayatlık satırı yalnız sayı varsa basılır             | B14 `tasks.` öneki    |
| `viewRelevant: boolean`     | Görünüm satırı basılsın mı (ölçüt §12 K7)                  | B14 `dataview.` öneki |

**`change` normalize girdiden üretilir (A2).** Sekme `change`'i, önizleme kararını, onay planını ve çalıştırmayı **aynı normalize ve doğrulanmış girdiden** verir. Bugünkü çekirdekte önizleme kararı `parseOptions()` sonrasında, normalizasyon sonra yapılıyor; normalizasyonun `yes: true` ürettiği komut onaysız çalışıyor (A2, doğrulandı). Bu hata §8 adım 0'da ayrıca düzeltilir.

`received` gelmeden kopmada CLI yetenek listesindeki `danger`'a bakar; `confirm`'u yorumlamaz. **[FARK]** Önizleme çağrısı `received` öncesi koparsa kod 4 yerine 5. Skill metni buna göre güncellenir (F21).

### 5.3 Komut adı ve hassas süzgeç (A3, A5, F1, F2, F5, F6, F7)

**Komut adı kuralı (iki giriş biçimi için tek):**

- `argv`: ad `argv[0]`.
- `line`: baştaki boşluk atlanır; ad, ilk boşluğa kadar olan parçadır.
- "Boşluk" = ECMAScript `WhiteSpace` + `LineTerminator` kümesi (U+FEFF dahil, U+0085 hariç), iki tarafta da aynı tablo.
- Ad `^[A-Za-z][A-Za-z0-9._-]*$` kalıbına uymalı; uymazsa istek gönderilmez, kod 2 "komut adı tırnaksız ve düz yazılır". Kural `argv[0]`'a da uygulanır (F2: `gecko "set parola x"` tek argümanı). **[FARK]** Tırnaklı ad reddedilir.
- Karşılaştırmalar **büyük/küçük harf duyarsızdır** (ASCII katlama), hem hassas listede hem üyelik denetiminde (A3, F1: çekirdek `toLowerCase()` ile eşliyor, CLI bugün birebir arıyor; `CLI.SECRET value` bugün de sızıyor olabilir, §8 adım 0).

**Sekmenin değişmezleri** (bunları değiştirmek sözleşme değişikliğidir, §6):

1. Çalışacak komut satırın ilk parçasıdır ve adla (alias dahil, harf duyarsız) tam eşleşmeyle bulunur.
2. Bir istek tek komut çalıştırır; zincirleme (`a ; b`), önek komutu, bağlama göre ad çözme yoktur.
3. Ad bitişi yukarıdaki boşluk kümesidir.

**Sınama.** `fixtures/remote/command-name.json` her satırda: girdi, CLI'nın bulduğu ad (ya da ret), sekmenin bulduğu ad. command-console-core'un `remote.spec`'i her satırı `attachRemote` üzerinden koşar ve hassas satırın hassas yoldan reddedildiğini doğrular (F6). Satırlar: harf büyüklüğü (`Set`, `SET`), boşluk türleri (U+FEFF, U+00A0, U+0085, U+2028, sekme karakteri), tırnak, kaçış, boş, alias.

**Kapsam sınırı.** CLI yalnız **ad tabanlı** hassasiyeti korur. Çekirdeğin satır içeriğine bakan `isSensitive(line)` seçeneği CLI'da süzülemez; bunu kullanan komutun satırı hub'a ulaşır, çalıştırmayı sekme engeller. frontendx bugün bu seçeneği kullanmıyor. Bu sınır skill ve README'ye yazılır (A5, F7).

**Sekmenin öneri kuralları (F3).** Öneri adayları hassas adları içermez. Hassas sayılan satırın cevabında `suggest` ve `message` boştur. `suggest` hiçbir zaman bilinmeyen komutun argümanlarını taşımaz.

### 5.4 Sekmeden gelen hazır satırlar (B3, B4, B11, B12, B17)

`finished` ve `help-result` iki isteğe bağlı alan taşır; ikisi de çalıştırılabilir bir **argv dizisi**dir, `gecko` öneki yoktur (F20):

| Alan                | Ne zaman                                                                   | Örnek                                  |
| ------------------- | -------------------------------------------------------------------------- | -------------------------------------- |
| `suggest: string[]` | Hata (yakın ad, konumsal düzeltme) **ve** önizleme (`--yes` eklenmiş hâli) | `["tasks.stop","--id","8812","--yes"]` |
| `next: string[]`    | Sayfalı sonuçta devam                                                      | `["tasks.list","--cursor","40"]`       |

- CLI bunları kendi kalıp cümlesiyle ve **kendi tırnaklamasıyla** basar ("şunu mu demek istediniz: gecko …", "çalıştırmak için: gecko …", "devamı: gecko …"). Tırnaklama kullanıcının kabuğu içindir (POSIX ya da PowerShell), sekmenin söz dilimiyle ilgisi yoktur; CLI'nın kendi işidir.
- `--json`'da alanlar dizi olarak geçer. **Hiçbir zaman kendiliğinden çalışmaz.**
- `outcome.line` (bugün yalnız `not-allowed`) `suggest`'e katılır ve kaldırılır (F13).
- "kullanım: gecko help <komut>" ipucunu CLI ekler: sekme hata cevabında (`finished` ve `help-result`) isteğe bağlı `help: "<ad>"` alanı verir (F20 ters bağ, K-h). Alan yoksa CLI ipucu basmaz.

### 5.5 Yardım (B6, B9, B16)

- `help-request` ham girdi taşır: `argv: string[]` (`help`'ten sonraki her şey). Hedef, ad alanı/komut ayrımı, `--cursor`/`--limit` yorumunu sekme yapar (A7, F10).
- İstek ne istediğini söyler: `want: "text" | "json"`. `text` istenirse `width?: number` (TTY genişliği).
- `help-result` üst düzeyde genel sonuç taşır (A8): `status: "ok" | "error"`, hatada `code`, `class`, `message`. Ayrıca `text?: string[]` ya da `result?: unknown` (istenen biçime göre, ikisi birden değil: A12, F20 boyut), `suggest?`, `next?`.
- `help-request`'i **command-console-core'un uzak girişi** karşılar; frontendx'teki `cliHelp.ts` silinir (F28). Öneri, konumsal düzeltme, yardım metni ve sınıf eşlemesi çekirdekte durur; böylece Node sahte sekme gerçek üretim kodunu koşar.
- Yardım tek zarfa sığmazsa sekme sayfalar (`next`); `text` en çok 12 KB.

### 5.6 Hata sınıfı ve çıkış kodu (B7)

- `finished.outcome` ve `help-result` hata durumunda `class` taşır: `failed` 1 · `usage` 2 · `denied` 3 · `not-run` 4 · `unknown` 5 · `cancelled` 1.
- `cancelled`: kullanıcı komutu tarayıcıda iptal etti; kod 1 (K3). 130 yalnız CLI'daki Ctrl+C içindir.
- Sekme hiçbir zaman 0, 6 üretemez; 0 yalnız `status: "ok"`'tan gelir.
- **Kod → sınıf tablosu** command-protocol'de fixture: bugünkü 18 `ErrorCode`'un hepsi için.
- **Bilinmeyen `class` değeri ya da eksik `class`:** kod 1 (F18). Eski tabloya düşülmez (F19).
- **CLI'nın kendi gözlemi baskındır (A9, F18):**
  - `not-run` yalnız `received` **gelmeden** verilen retlerde kabul edilir. `received.change === true` görüldükten sonra gelen `not-run` → 5.
  - Değiştiren istekte (`received.change` ya da `danger: change`) çelişkili ya da eksik cevap → 5.
- `--json`'daki `status: "unknown"` ve `complete` sınıftan türetilir (F18).

### 5.7 Sürüm, özellik ve uyumluluk (A10, F15, F19, F24, F25)

**Kural 1: sessiz düşme yok (F15).** Sekme, `id`'sini okuyabildiği her bozuk isteğe `finished` hata cevabı döner (`invalid-args`, `class: usage`). Bugün okuyamadığı isteği sessizce bırakıyor (`dispatchFeature.ts:1111-1117`); CLI 32 sn bekleyip yanlışlıkla "ulaşmadı" diyor.

**Kural 2: taban özellik kümesi (F19).** Yetenek listesinin baş bilgisi (§5.8) `features: string[]` taşır. Rust CLI'nın taban kümesi: `argv`, `received-v2` (§5.2 alanları), `suggest-argv`, `help-v2`, `error-class`, `capabilities-v2` (§5.8 biçimi). Sekme bunlardan birini bildirmiyorsa CLI tek bir şey yapar: kod 2 `unsupported-version`, "Gecko sekmesi bu CLI için eski; sayfayı yenileyin ya da ortamın güncellenmesini bekleyin". Eski yollar Rust'a **yazılmaz**: uyumlu kalınacak canlı ortam yok, SignalR iki tarafta da iskelet.

**Kural 3: çift yönlü sürüm aralığı (A10, F24, F25).**

- `envelopes: number[]` (baş bilgide, bugünkü `envelope: 1` yerine). CLI ortak en yüksek sürümü seçer ve isteğine yazar (`v`).
- `minCli?: string` (baş bilgide): sekmenin desteklediği en eski CLI sürümü. CLI altındaysa kurulum komutunu basar, kod 2.
- CLI sürümünü hub'a bağlanırken `User-Agent` ile bildirir (`gecko/<sürüm> (<platform>)`).

**Kural 4: bilinmeyeni yok say, enum'a değer eklemek tanımlı varsayılan ister (F24).**

- İki taraf tanımadığı alanı yok sayar. CLI tanımadığı cevap `kind`'ını yok sayar.
- Enum'a değer eklemek, okuyucunun varsayılanı tanımlı değilse kırıcıdır. Tanımlı varsayılanlar: bilinmeyen `danger` → `change`; bilinmeyen `outcome.status` → `error` + kod 1; bilinmeyen `class` → 1.
- Rust tarafında enum'lar `#[serde(other)]` taşır.

**Tek yönlü `features` yeterli** (F25): her `gecko` çağrısı yeni bağlantı, sekme CLI durumu tutmaz; sekmenin CLI'ya göre davranması gereken yerde istek kendi isteğini taşır (`want`).

### 5.8 Yetenek listesi (A5, A12, F22)

- CLI listeyi üç iş için kullanır: üyelik denetimi (§5.2), hassas süzgeç (§5.3), `received` öncesi kopmada `danger`. `confirm` okunmaz.
- **Yol (S1, 2026-11-10, frontend-v2-design ile).** Liste zarfla gitmez. Sekme onu hub yöntemiyle yayımlar: `publishCapabilities { revision: string, parts: string[] }`. Hub bunu yalnız bağlı tarayıcı bağlantısından kabul eder, listeyi **atomik** değiştirir (eski parçalarla yeni parçalar karışmaz) ve içeriğe bakmaz (§6.6). CLI listeyi `status` cevabından okur: `binding.capabilities = { revision, parts }`. `capabilities-chunk` türü yoktur.
- **Biçim.** Liste her zaman parçalıdır (tek parça da bir dizidir). `parts` birleştirilince tek JSON nesnesi olur; baş bilgi bu nesnenin başındaki alanlardır (`features`, `envelopes`, `minCli`, `revision`), komutlar sonra gelir. Parça eksik ya da birleşim çözümlenemiyorsa CLI listeyi yok sayar ve liste hiç yokmuş gibi davranır (aşağıdaki kural: kod 4 `tab-unreachable`).
- **Güncellik (A5).** `revision` sekme komut kümesi ya da hassas liste değiştikçe değişir; sekme yeni listeyi yeniden yayımlar. CLI dosya önbelleğini **hassas süzgeç için kullanmaz**: hub'da liste yoksa istek gönderilmez, kod 4 `tab-unreachable` (F22). **[FARK]** Bugün dosya önbelleği yedek olarak kullanılıyor.
- **Boyut (A12).** 300 komutlu liste 17 KB oldu ve reddedildi. Tek parça sınırı 16 KB, en çok 3 parça. Hub'ın SignalR `MaximumReceiveMessageSize` değeri en az 64 KB olur (#1974). Liste yalnız gerekenleri taşır: `name`, `aliases`, `danger`, `sensitive` adları.

### 5.9 Sekmenin konumu (B13)

`gecko status` ve `attach` sonrası "Bağlandı: <başlık> · <yol>" satırı bugün `nav.where` komutunu çalıştırıyor. Yerine sekme konumunu hub yöntemiyle yayımlar: `publishWhere { title, path }` (S2). `path` yalnız yoldur; sorgu dizesi ve `#` sonrası gönderilmez (adreste kimlik ya da filtre değeri olabilir). Yöntemi yalnız frontendx çağırır, hub yalnız bağlı tarayıcı bağlantısından kabul eder, saklar ve `status` cevabında `binding.where` olarak verir; içeriğini okumaz. Sekme geçmişinde `nav.where` satırı da kalkar.

## 6. Sözleşme değişiklik kuralı (A10, A11, F16, F23)

1. **Ekleme serbest:** isteğe bağlı alan, `features` adı, hata kodu (`class`'ıyla, tabloya satırıyla). Enum değeri yalnız §5.7 kural 4'teki tanımlı varsayılanla.
2. **Yeni zarf türü** bir `features` adıyla gelir.
3. **Kırıcı değişiklik** `envelopes` listesine yeni sürüm demektir. Eski sürüm, `minCli` ile duyurulan bir takvimle kalkar; en az 90 gün iki sürüm birlikte konuşulur.
4. **Yayınlanmış fixture satırı değiştirilemez ve silinemez, yalnız eklenir.** command-console CI'ı son yayınlanan arşivle fark alıp bunu zorlar (F23).
5. **Kapı değişikliği yapan tarafta (F23).** command-console PR CI'ı yayınlanmış son `gecko` ikilisini indirip PR'ın çekirdeğine karşı duman sözleşme testleri koşar. gecko-cli PR CI'ı yayınlanmış son command-console-core'a karşı sözleşme testlerini koşar. Haftalık iş ayrıca en yeni çekirdeği ve en yeni fixture'ları dener (F26).
6. **Hub opaktır (F16, #1974'e yazıldı).** Hub zarfı türüne bakmadan damgalar ve yönlendirir; `kind`'ı yalnız hız sınırı için okur (`cancel` sayılmaz). Bilinmeyen `kind` geçer. Yeni zarf türü backend değişikliği gerektirmez.

## 7. Rust tarafı

### 7.1 Kapsam

- **Ürün:** `gecko` ikilisi, %100 Rust; iki taşıma (dev ve SignalR).
- **`gecko dev-hub`:** **[Karar]** Rust'a taşınır mı, test aracı olarak Node'da mı kalır (F32)? Öneri aşağıda §12'de.
- **Test düzeneği Node'da kalır** (A13, F27): sahte sekme gerçek `command-console-core` `attachRemote`'unu koşar. Bugün `src/tab/index.ts`, `src/transport/dev.ts`, `src/shared/rpc.ts`, `src/shared/wire.ts` dosyalarını kullanıyor; bunlar `tests/support/` altına taşınır, ürün kodu sayılmaz.
- **Test kancaları (F32)** sözleşmenin parçasıdır ve `docs/test-hooks.md`'de listelenir: `GECKO_TEST_INTERRUPT_STDIN`, `GECKO_TEST_TTY`, `GECKO_DELIVERY_MARGIN_MS`, `GECKO_CANCEL_WAIT_MS`, `GECKO_CAPABILITIES_WAIT_MS`, `GECKO_STATUS_WAIT_MS`, `GECKO_TRANSPORT`, `GECKO_DEV_HUB_*`. Rust ikilisi bunları aynen okur.
- **Birim test senaryoları** Rust testlerine taşınır; silinen işlevlerinkiler (B3–B5) command-console-core'a taşınır.

### 7.2 Yapı

```
gecko-cli/
  Cargo.toml              # workspace
  crates/
    gecko/                # ikili: main, argüman, çıktı, akış
    gecko-hub-client/     # Transport trait, dev (ws), signalr
    gecko-wire/           # zarflar (serde), fixture testleri
    gecko-dev-hub/        # §12 kararına bağlı
  fixtures/hub/
  docs/hub-protocol.md, docs/test-hooks.md
  tests/contract/, tests/support/   # Node
  install/                # install.ps1, install.sh
```

`Transport` trait'i kapanış nedenlerini anlamsal taşır (`Closed::Revoked`, `Closed::Busy`, `Closed::Unauthorized`), kapanış kodunu değil (B18, F14). Hub hataları sonuç değeri olarak döner (#1974'e yazıldı).

### 7.3 Kütüphane seçimleri

| İş         | Seçim                                                                          | Not                                                                                                                         |
| ---------- | ------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------- |
| Async      | `tokio` current-thread                                                         | Keep-alive, Ctrl+C, zaman aşımı aynı anda                                                                                   |
| WebSocket  | `tokio-tungstenite`                                                            |                                                                                                                             |
| TLS        | `rustls` + `ring` + `rustls-platform-verifier`                                 | İşletim sistemi güven deposu: kurum içi sertifika otoritesi çalışır (F30). Windows/musl'da C araç zinciri derdi azalır      |
| Vekil      | `HTTPS_PROXY`/`NO_PROXY` hem HTTP hem WebSocket için aynı kural                | `reqwest` sistem vekilini kullanır, `tokio-tungstenite` kullanmaz (F30); WebSocket için `CONNECT` tünelini kendimiz kurarız |
| HTTP       | `reqwest` (`rustls`, `json`)                                                   |                                                                                                                             |
| JSON       | `serde_json` + `preserve_order`; `data` için `RawValue`                        | Alan sırası tablo sütun sırasıdır (F31)                                                                                     |
| ASCII JSON | Özel yazıcı: UTF-16 birimleriyle `\uXXXX`, BMP dışı vekil çift, U+007F kaçışlı | Fixture'a emoji satırı (F31)                                                                                                |
| Argüman    | `std::env::args_os`, elle ayrıştırma                                           | Geçersiz Unicode'da çökmez (F31)                                                                                            |
| Ad kalıbı  | elle bayt denetimi                                                             | `regex` yok                                                                                                                 |
| SignalR    | Elle yazılmış dar istemci (`tokio-tungstenite` üstünde)                        | §7.4 denemesi, [signalr-trial.md](signalr-trial.md). TLS akışı kendimiz açarız; HTTP ile aynı `rustls` yapılandırması       |

### 7.4 SignalR (A14, F29)

Crate durumu (2026-11-10): `signalr-client` 0.3.2 (2026-03-03, tek yazar, yeniden bağlanma API'si var, `tokio-native-tls` bağımlılığı); `signalrs-client` 0.1.1 (2023); `signalr_rs` 0.2.6 (2022); resmî istemci yok.

Kullanım dar (çağrı başına tek kısa bağlantı, JSON, WebSocket, akış yok, yeniden bağlanma gerekmez); elle yazım makul görünüyor, ama **karar denemeyle verilir.** §8 adım 4a:

- En küçük bir ASP.NET Core hub'ı (`tests/signalr-hub/`, CI'da kalır; gerçek backend bizde değil).
- Aday: `signalr-client` ve elle yazılmış dar istemci.
- Sınananlar: `negotiate` (yönlendirme `url` + `accessToken` dahil) ve `skipNegotiation`; yükseltmede `Authorization` başlığı; el sıkışma `error` alanı ve ilk mesajlarla aynı çerçevede gelmesi; tek WebSocket mesajında çok kayıt; 2 dk boşta bekleme ve `Ping`; `Close` (`error`, `allowReconnect`); `Completion` hatası; sunucunun sonuç bekleyen çağrısı; tanınmayan mesaj tipleri (8, 9) yok sayılır; sunucu taraflı kapatma.
- **Yeniden bağlanmada komut otomatik tekrar gönderilmez.**
- Desteklenmeyenler açıkça yazılır: MessagePack, long polling, SSE, stateful reconnect, akış.

**Sonuç (2026-11-10, [signalr-trial.md](signalr-trial.md)):** elle yazılmış dar istemci seçildi. `signalr-client` token'ı WebSocket adresine `access_token=` olarak koyduğu için hub'a bağlanamadı; ayrıca `skipNegotiation`, `Close` nedeni, istemci `Ping`'i ve el sıkışma hatası onda yok. Elle istemci yukarıdaki her maddeyi geçti (9 birim, 12 entegrasyon testi, CI'da Windows ve Ubuntu). Kapanış nedenleri `Closed::{Unauthorized, Busy, Revoked, Handshake, Server, ServerTimeout, Transport, Local}`; hub sonucu `{ ok, value | error }` ayrı bir adımda çözülür. İstemci adım 4b'de `gecko-hub-client`'e taşınır.

### 7.5 Dağıtım (F33)

- Hedefler: `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-apple-darwin`. Her ek SHA-256 ile.
- macOS ikilileri macOS runner'da derlenir, en az ad-hoc imzalı.
- `install.ps1` / `install.sh`: son sürümü indirir, SHA-256 doğrular, `%LOCALAPPDATA%\gecko\bin` / `~/.local/bin`'e koyar, PATH'e ekler. Repo public olunca token gerekmez.
- İmzasız Windows ikilisinde antivirüs yanlış pozitifi beklenir; README'de yazılır. İmzalama, `winget`, `brew`, otomatik güncelleme kapsam dışı.

## 8. Sıra

**Adım 0 — bugünkü hatalar (Rust'tan bağımsız, hemen):**

- 0a. command-console: önizleme/`change` kararı normalize girdiden (A2). Değiştiren komut onaysız çalışabiliyor.
- 0b. gecko-cli: `received` gelmeden süre dolunca, değiştiren istekte kod 4 yerine 5 (A1). Bugün iki kez çalışmaya yol açabilir.
- 0b **yapıldı**: gecko-cli#10, PR #11 merge (2026-11-10). 0a → command-console#23, 0d → command-console#24.
- 0c. **Bugün hata değil, düşürüldü:** büyük harfli ad yetenek listesinde bulunmadığı için CLI onu bilinmeyen komut olarak yerelde durduruyor, satır gönderilmiyor. Kural Rust tasarımında geçerli (§5.3).
- 0d. command-console: okunamayan isteğe sessiz düşme yerine hata cevabı (F15).
- 0e. #1974 güncellendi (hub opak, yapılı hata, kapanış biçimi). **Yapıldı**, 2026-11-10.

**Adım 1 — command-protocol 1.7.0:** §5'teki alanlar, `features`, `envelopes`, `minCli`, `revision`, normatif tablo, fixture dizini (dört sınıf) ve yayın; kod → sınıf tablosu; değişmez kontrolü CI'ı (§6.4).

**Adım 2a — command-console-core:** `argv` yolu, `received` alanları, `suggest`/`next`/`help`, `help-request` karşılayıcısı ve yardım metni, sınıf, `where`, bölünmüş yetenek listesi. Sürüm.

**Adım 2b — frontendx:** çekirdek yükseltmesi (`package.json` tam sürüme sabit; kendi yayın hattı), `cliHelp.ts` silinir, `where` bildirimi. Ayrı PR, ayrı takvim (F34).

**Adım 3 — gecko-cli (TypeScript):** yeni özellikleri kullanır, B1–B18 kodunu siler, sözleşme testleri ve [FARK] satırları güncellenir. Rust'ın taşıyacağı davranışın son referansı. Test yardımcıları `tests/support/`'a taşınır.

**Adım 4a — SignalR denemesi** (§7.4). `Transport` trait'inin biçimini belirler; Rust işinden önce. **Yapıldı** (2026-11-10): elle yazılmış dar istemci, [signalr-trial.md](signalr-trial.md).

**Adım 4b — Rust CLI**, mevcut Node hub'ına karşı: sözleşme testleri, argüman matrisi (DC-3), ölçümler. Workspace iskeleti **yapıldı** (2026-11-10): dört crate, `args_os` ayrıştırması, `--help`/`--version`, `dev-hub` özelliği, ASCII JSON yazıcısı, Windows/Ubuntu/macOS CI'ı ve `gecko-dev` artefaktı. Ölçüm: `--help` p50 3,5 ms, sürüm ikilisi 115 KB (Windows 11, 20 örnek). 4b-1 **yapıldı** (2026-11-10): SignalR istemcisi `gecko-hub-client`'te; vekil tüneli, kısaltılmış HTTP hata metni, çağrı başına cevap süresi ([signalr-trial.md](signalr-trial.md)). 4b-2 **yapıldı** (2026-11-10): geliştirme taşıması (`dev.rs`) ve taşıma seçimi; `Transport` trait'i Node'daki yöntem ve olaylara genişledi (`pair`, `detach`, `HubEvent`). 4b-3 **yapıldı** (2026-11-10): `login` (RFC 8628), `logout`, token yenileme, ortam çözümü, config dosyaları Node biçiminde (Rust'ın yazdığı token'la Node CLI hub'a bağlandı), `User-Agent` `gecko/<sürüm> (<os>; <arch>)`. Ölçüm: sürüm ikilisi 2,4 MB, `--help` p50 4,5 ms. 4b-4 **yapıldı** (2026-11-10): `attach [--new]`, `detach`, `status`; oturum açılışı (reddedilen token bir kez yenilenir), eşleştirme bekleyişi (hub saatine göre en çok 2 dk, `other-account` yalnız not, Ctrl+C 130). Sekmenin adı hub'ın `binding.where` alanından okunur (§5.9), `nav.where` çağrılmaz; `attach` yetenek listesini beklemez. `status` onay zamanını göreli yazar ("approved 7 min ago"); `--json` Node'un `data` biçimini taşır, `title` ve `path` `untrusted` listesindedir. SignalR hub'ı henüz bağlanmaz: localhost dışı ortamda komut "use the Node build" der. Node dev hub'ına karşı elle denendi: Rust `login`, sahte sekmeyle `attach`, `status`, `detach`. Dev hub `publishCapabilities` ve `publishWhere`'i tanıyor (2026-11-10, gecko-cli#20). 4b-5a **yapıldı** (2026-11-10): command-console 1.7.0 sözleşmesi `gecko-wire`'da (cevap okuyucu, istek zarfları, komut adı kuralı, sınırlar, parçalı yetenek listesi); 1.7.0 yayın ekinin `valid`, `invalid`, `future`, `error-class` ve `command-name` örnekleri Rust'ta koşuyor. 4b-5b **yapıldı** (2026-11-10): satır yolu. Argv, `--line` ve `--line -` sekmeye gider; hassas ad gönderilmez (3), listede olmayan addan yalnız ad `help-request` ile sorulur ve sekmenin önerisi basılır; eski sekme (`kind: capabilities`, eksik `features`, `minCli`) 2 döner. `rate-limited` toplam 5 sn'ye kadar yeniden denenir. Teslim süresi hub'ın `expiresAt - stampedAt` penceresi artı pay; Ctrl+C `cancel` gönderir, cevabı 3 sn beklenir. 1.7.0'ın 18 `sequence/` örneği Rust'ta koşuyor. Düz çıktı önizleme satırını, `More:` ve `Did you mean:` satırlarını Windows'ta PowerShell'e, diğer sistemlerde POSIX kabuğuna göre tırnaklar; veri şimdilik girintili JSON, tablo biçimi ve `--out` 4b-5c'de.

**Adım 4c — çapraz matris** (§12 kararı Rust hub ise): Rust hub ↔ TypeScript CLI, Rust hub ↔ frontendx `devHubTransport`, sonra Rust ↔ Rust. TypeScript silinince bu kanıt bir daha üretilemez (F34).

**Adım 4d — uçtan uca kapı:** Rust ikilisi gerçek frontendx'e (yeni çekirdekle) ve geliştirme hub'ına karşı.

**Adım 5 — ilk Rust yayını ve TypeScript ürün kodunun silinmesi.** İlk yayın **yalnız geliştirme hub'ıyla** çalışır; bu README'de açıkça yazılır (A15). Kurulum betikleri. README bu adımda İngilizce ve kısa olarak yeniden yazılır (K6).

**Adım 6 — repo public** (A15: backend'e bağlanmaz). Ön koşullar (F33): lisans (`LICENSE`, `UNLICENSED` kalkar), private repolara bağlantılar temizlenir (README, `docs/`, `CHANGELOG.md`, `AGENTS.md`), git geçmişi gizli bilgi için taranır.

**Adım 7 — SignalR taşıması**, #1974 gelince; ayrı teslimat.

Üst belge güncellemesi (frontend-v2-design) adım 3 ile birlikte.

## 9. Başarı ölçütü

- Hiçbir **davranış senaryosu** kaybolmaz (A13, F36): sözleşme testlerinin her senaryosu Rust ikilisine karşı yeşil; silinen birim testlerinin senaryoları Rust'a ya da command-console-core'a taşınmış; [FARK] satırları güncellenmiş.
- gecko-cli'nın `command-protocol` / `command-console*` paketlerine **ürün** bağımlılığı sıfır (test bağımlılığı kalır).
- §1'deki yalıtım ölçütünün her maddesi **ayrı ayrı** kasıtlı denenir; ardından §6.5'teki CI bunu sürekli yapar.
- Performans, makine ve örnek sayısıyla (A16): Windows 11, ölçüm betiği aynı, 20 örnek.
  - Başlangıç: `gecko --help` (yerel) < 20 ms. Bağlı sekmeden `gecko help` ayrı ölçülür.
  - Tek çağrı, hız sınırı altında: okuma komutu p50 ≤ 85 ms. **Gerilememe ölçütüdür**, kazanç hedefi değil: süre hub turuyla sınırlı.
  - Hız sınırına giren sürekli yük: 100 ardışık çağrı hatasız (bugün p50 183 ms, sınırdan).
  - Hub kapalıyken kod 4 ≤ 76 ms.
  - İkili: sıkıştırılmamış < 10 MB.
- Windows'ta PowerShell 5.1 / 7, `cmd`, Git Bash; Linux; macOS'ta kurulum betiği ve `gecko help` çalışır.

## 10. Kapsam dışı

Anahtar deposu (token dosyada kalır) · kurulumlu masaüstü paketi (imza, `.msi`, otomatik güncelleme) · gerçek hub'ın backend tarafı · sekme tarafının Rust/WASM'a taşınması · satır içeriğine bakan hassasiyetin (`isSensitive`) CLI'da süzülmesi.

## 11. İnceleme bulgularının durumu

İki incelemenin bütün MUST bulguları bu taslağa işlendi. Kabul edilmeyen ya da değiştirilerek alınanlar:

| Bulgu                       | Durum                                                                                                                                           |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| F4 uzaklık süzgeci          | Gerek kalmadı: §5.2 üyelik denetimi argümanları hiç göndermiyor                                                                                 |
| A4 "önce yalnız adı gönder" | Kabul, `help-request` yoluyla (F4'ün önerisiyle birleşti)                                                                                       |
| F19 geri düşüş yok          | Kabul; A10'un "eski sekmeye yalnız `unsupported-version` demek yetmez" itirazı `minCli` ve `envelopes` ile karşılandı, eski yol yine yazılmıyor |
| F20 dil                     | Açık: §12                                                                                                                                       |
| F32 `dev-hub`               | Açık: §12                                                                                                                                       |

## 12. Verilen kararlar (2026-11-10, Zahid)

| #   | Karar                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| K1  | `gecko dev-hub` **Rust'a taşınır** (`gecko-dev-hub` crate'i). Yerleşimi Claude'a bırakıldı; karar: `dev-hub` cargo özelliğinin arkasında. Sürüm ikilisi (`release` iş akışı) özelliği **kapalı** derler: müşteriye HTTP sunucusu ve `--auto-approve` gitmez; `gecko dev-hub` orada "this build has no dev hub" der, kod 2. Geliştirici ve CI derlemeleri özelliği açar; CI ayrıca `gecko-dev` adlı ikiliyi artefakt olarak üretir (Release'e eklenmez). Node yalnız sözleşme testlerinin sahte sekmesinde kalır |
| K5  | Merge yetkisi verildi (2026-11-10, Zahid)                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| K2  | **Bütün kullanıcı metinleri İngilizce**: CLI'nın kendi metinleri, sekmenin CLI'ya döndüğü mesaj, yardım ve öneriler. **[FARK]** Bugün CLI metinleri Türkçe. `help-request`'te dil alanı yok. README ve skill örnekleri buna göre güncellenir. Kod 1–6 ve 130 anlamları değişmez                                                                                                                                                                                                                                 |
| K3  | Tarayıcıda iptal edilen komut: `class: cancelled` → **kod 1**. **[FARK]** Bugün 130. 130 yalnız CLI'daki Ctrl+C                                                                                                                                                                                                                                                                                                                                                                                                 |
| K4  | Adım 0 hataları spec onayından sonra issue olarak açılır; hızlı düzeltilip merge edilir                                                                                                                                                                                                                                                                                                                                                                                                                         |
| K6  | README **İngilizce ve kısa** (2026-11-10). Repo müşteriye açılacağı için; kurulum, giriş, bağlanma, örnek komutlar ve çıkış kodları. Ayrıntı `docs/`'ta kalır. Ürün Rust'a geçince (adım 5) yazılır                                                                                                                                                                                                                                                                                                             |
| K7  | `viewRelevant` (K-e, 2026-11-10): komut tanımında açık `usesView` bayrağı, namespace düzeyinde de konabilir (frontendx `dataview`'a bir kez koyar; komut değeri namespace değerini ezer). Alan = bayrak VEYA istek `view` taşıyor; açık görünümün varlığı tek başına true yapmaz, bayrak yoksa false                                                                                                                                                                                                            |

**Uygulayıcılar arası anlaşmalar (2026-11-10, gecko-cli ↔ command-console oturumları; Zahid kararı değil, tasarım ayrıntısı):** S1 yetenek listesi `publishCapabilities` yöntemiyle (§5.8) · S2 konum `publishWhere` yöntemiyle (§5.9) · S3 command-console `fixtures/remote/sequence/` örneklerinin beklenen kodları gecko-cli tarafının onayıyla merge edilir · K-h hata cevabında `help` alanı (§5.4).

## 12b. Eski sorular (kayıt için)

1. **`gecko dev-hub` Rust'a taşınsın mı?** 1174 satır; WebSocket/HTTP sunucusu, CORS, cihaz girişi sayfaları, `--auto-approve`. Geçici bir araç ("backend gelene kadar").
   - a) Node'da test ve geliştirme aracı olarak kalır, ürün ikilisine girmez. **Öneri.** Ürün ikilisi küçük kalır, son kullanıcıya HTTP sunucusu ve otomatik onay dağıtılmaz. Ama geliştirme hub'ını kullanmak Node ister; "Node'suz" yalnız ürün için geçerli olur.
   - b) Rust'a taşınır, sürüm ikilisinde kapalı bir cargo özelliği olarak; geliştirici derlemesi açar.
   - c) Rust'a taşınır, ürün ikilisinde açık.
2. **Dil.** Çekirdeğin mesajları İngilizce, frontendx metinleri kullanıcının dilinde, CLI metinleri Türkçe sabit. Sekmeden gelen yardım ve hata metniyle karışık çıktı oluşur. Müşteri kullanacağı için:
   - a) CLI de kullanıcının dilinde (`LANG` / `GECKO_LANG`, Türkçe ve İngilizce); `help-request` dili taşır. **Öneri.**
   - b) Her şey Türkçe; sekme CLI'ya Türkçe yazar.
3. **Tarayıcıda iptal edilen komutun çıkış kodu:** 130 mı 1 mi? Öneri 1.
