# Changelog

## 0.1.0 (yayımlanmadı)

İlk sürüm: masaüstü CLI spec'inin (3. taslak) uygulaması.

- `gecko` CLI'ı: `login`, `logout`, `attach [--new]`, `detach`, `help`, `dev-hub`; diğer her satır bağlı sekmeye.
- Geliştirme hub'ı (`@dolusoft/gecko-cli/dev-hub`): cihaz girişi, CLI token'ı, bağlama, damga, yönlendirme, hız sınırı, iptal.
- Tarayıcı tarafı istemcisi (`@dolusoft/gecko-cli/tab-client`) ve [docs/browser-side.md](docs/browser-side.md).
- `Transport` arayüzü; SignalR taşıması iskelet.
- Düz metin `gecko help <komut>` `input` şemasını kesmeden basar: alan başına bir satır (tip, zorunlu, enum/min/max/maxItems), açıklama terminal genişliğinde kaydırılır.
- `gecko help` listesi ve `gecko --help` altında "CLI seçenekleri komut adından önce gelir" notu.
- `nextCursor` dönen sonuçlarda düz metin sonuna `devamı: gecko <satır> --cursor N`.
- `gecko dev-hub` `--web` verilmeden başlatılınca loopback kökenlere (`http://127.0.0.1:*`, `http://localhost:*`, `http://[::1]:*`) CORS izni verir; `--web` ile yalnız o köken. Başlangıçta `cors:` satırı izinli kökenleri yazar.
- Hız sınırı reddi `busy` değil `rate-limited` (`retryAfterMs` ile); CLI bu süre kadar bekleyip yeniden dener, toplamda en çok 5 sn. Tek canlı bağlantı `busy`'si yeniden denenmez.
- `gecko help nope.nope` (listede olmayan `ad.ad`) kod 2 `unknown-command`; `--line "help …"` ya da stdin'den gelen `help` satırı da sekmenin `help` komutuna değil `help-request` yoluna gider.

### Spec'ten sapmalar

- `gone` nedenlerine `detached` eklendi: `gecko detach` ve terminaldeki "bağlantıyı kes" düğmesi `closed`'dan ayrılır.
- Hub, sekmenin son `capabilities` cevabını bağlama başına saklar ve CLI bağlanınca `status` ile verir; her `gecko` çağrısı yeni bağlantı olduğu için başka yolu yok. Hub zarfın yalnız `kind` alanını okur.
- `received` gelmeden Ctrl+C: §5.5 "kod 5" diyor, §6.6 "130". CLI 130 ile çıkar, `--json` `code: "unknown"` yazar.
- `gecko help` girişsizken CLI'nın kendi yardımını basar (kod 0); girişliyken bağlı sekmenin ajan `help` aracını kullanır: satır değil, ayrı bir `help-request` zarfı yollanır, sekme `help-result` ile cevap verir ([docs/browser-side.md](docs/browser-side.md)). Sekmenin kendi `help` komutu CLI'ya açık olmadığı için (`not-allowed`) satır yolu çalışmıyordu.
- `--line "help"` satırı yetenek listesinde olmasa da sekmeye gider; karar sekmenindir.

### 1. deneme turu düzeltmeleri

- Kendi komutlarında `--help`/`-h` o komutun kullanımını basar; `gecko dev-hub --help` artık hub başlatmaz. `gecko <komut> --help` sekmenin o komut için yardımıdır.
- Düz metin çıktıda `data` ham JSON basılmaz: satır dizisi ve nesnenin satır dizisi alanları (`items`) tablo olur (sütunlar ilk 20 satırın anahtarlarından, hücre 40 karakterde kesik, terminale sığmayan sütunlar adıyla bildirilir); diğer alanlar `anahtar: değer`. `--json` değişmedi.
- Yetki iptali komut sürerken gelirse sonuç kod 6 (`revoked`), kod 4/5 değil.

### 2. deneme turu düzeltmeleri

- Hız sınırı token bucket oldu: yetki başına ortalama 5 istek/sn, 20'lik patlama. Yalnız sekmeye giden işler (`cli-request`, `help-request`) sayılır. Ardışık `gecko` döngüsü (~16 çağrı/sn) artık 20 çağrıda `busy` almaz; eşzamanlı yağmur yine sınıra takılır. Patlama `GECKO_DEV_HUB_BURST` ile değişir.
- Hub tarayıcı bağlantılarına heartbeat yollar (4 sn'de bir `ping`, iki cevapsız `ping` kopma sayılır): ağı kesilen ama açık görünen sekmede CLI ~32 sn bekleyip `not-delivered` almak yerine 8–12 sn içinde `tab-unreachable` (kod 4) alır; `pong` dönünce bağlama `active` olur.
- Düz metin çıktıda aynı nesnede `columns` (metin dizisi) ve `rows` (dizi dizisi) varsa (`dataview.rows`) tek tablo basılır, başlıklar `columns`'tan; diğer alanlar `anahtar: değer`.

#### Spec'ten ve brifingten sapmalar

- Patlama 10 değil 20: tek `gecko` çağrısı yerelde ~60 ms sürüyor, 5/sn + 10 patlama ile 20 ardışık çağrının son 4-5'i `busy` alıyordu. Ortalama 5/sn (§5.1) aynı.
- Ping aralığı 5 değil 4 sn: 5 sn ile tespit 10–15 sn arası, ölçümde 15,08 sn çıktı; 4 sn ile en kötü 12 sn.
