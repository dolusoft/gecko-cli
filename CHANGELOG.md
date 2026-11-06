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
- `gecko help` girişsizken CLI'nın kendi yardımını basar (kod 0); girişliyken spec'teki gibi bağlı sekmenin `help`'ini kullanır.
- `help` satırı yetenek listesinde olmasa da sekmeye gider; karar sekmenindir.
- Yetki iptali komut sürerken gelirse sonuç kod 6 (`revoked`), kod 4/5 değil.
