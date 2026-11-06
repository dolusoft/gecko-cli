# Ajan rehberi

Bir şeyi değiştirmeden önce oku. Kısa tutuldu; ayrıntı bağlantılı dosyalarda.

## Ne bu repo

`gecko` CLI'ı ve masaüstü CLI spec'inin (frontend-v2-design `design/desktop-cli.md`, 3. taslak) §10.3 sözleşmesini uygulayan geliştirme hub'ı. Spec kanoniktir; buradaki davranış ondan sapıyorsa sapma PR'da ve [CHANGELOG.md](CHANGELOG.md)'de yazılır.

## Düzen

- `src/cli/`: CLI. `main.ts` giriş; `args.ts` seçenekler (komut adından önce, §10.1); `run.ts` bir satırın yolu (§4, §5.5, §6.6); `output.ts` çıktı ve çıkış kodları (§6); `session.ts` bağlantı, bağlama, yetenek listesi; `auth.ts` cihaz girişi; `config.ts` dosyalar ve token izinleri.
- `src/transport/`: `Transport` arayüzü. `dev.ts` geliştirme taşıması (yalnız localhost), `signalr.ts` iskelet (#1974 bekleniyor), `select.ts` seçim.
- `src/hub/`: geliştirme hub'ı, `@dolusoft/gecko-cli/dev-hub` olarak dışa açık.
- `src/tab/`: tarayıcı tarafı istemcisi, `@dolusoft/gecko-cli/tab-client`. Tarayıcıda da çalışır; Node'a özgü modül almaz.
- `src/shared/`: çerçeve ve RPC; tarayıcıda da çalışır.
- `tests/contract/`: hub ve CLI gerçek süreç, sahte sekme command-console-core'un `attachRemote`'u. Spec tablolarının her satırı bir test.
- `tests/unit/`: saf işlevler.

## Kurallar

- Çalışma zamanı bağımlılığı yalnız `@dolusoft/command-protocol` ve `ws` (yalnız hub, tembel yüklenir). Küçük paketi kendimiz yazarız.
- CLI'nın açılış yolu ağ ve hub modülü yüklemez; `pnpm measure:startup` `gecko help` için 300 ms sınırını denetler.
- `stdout` yalnız sonuç; bilgi `stderr`'e. `--json` ASCII.
- Token yalnız `Authorization` başlığında; asla adreste.
- Kod ve kod yorumları İngilizce; README, commit mesajı, PR Türkçe. Makine anahtarları İngilizce.

## Dallar ve PR

- `main` tek uzun ömürlü dal; iş PR ile gelir, PR gövdesi Türkçe.
- Force push yok. Commit'ten sonra push.
- AI co-author satırı yok.

## Push etmeden önce

`pnpm lint && pnpm typecheck && pnpm build && pnpm test`. İş ortasında yalnız değişen testler; PR öncesi bir kez tam paket.
