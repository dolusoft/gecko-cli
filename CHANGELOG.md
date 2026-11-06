# Changelog

## 0.1.0 (yayımlanmadı)

İlk sürüm: masaüstü CLI spec'inin (3. taslak) uygulaması.

- `gecko` CLI'ı: `login`, `logout`, `attach [--new]`, `detach`, `help`, `dev-hub`; diğer her satır bağlı sekmeye.
- Geliştirme hub'ı (`@dolusoft/gecko-cli/dev-hub`): cihaz girişi, CLI token'ı, bağlama, damga, yönlendirme, hız sınırı, iptal.
- Tarayıcı tarafı istemcisi (`@dolusoft/gecko-cli/tab-client`) ve [docs/browser-side.md](docs/browser-side.md).
- `Transport` arayüzü; SignalR taşıması iskelet.

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
