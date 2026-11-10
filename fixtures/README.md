# Fixtures

`command-console-<sürüm>/` klasörleri command-console sürümünün yayın ekinden (`remote-fixtures-<sürüm>.tar.gz`) **olduğu gibi** kopyalanır: `remote/` örnekleri ve `remote-contract.md` sözleşmesi. Elle düzenlenmez, biçimlendirilmez (`.prettierignore`). Yeni sürüm yeni klasörle gelir; protokol biçimi değişmediyse eski klasör kalkar (1.7.1 yalnız örnek ekledi: 1.7.0 örnekleri aynen içinde).

| Klasör                   | Kaynak                                                    | Arşivin SHA-256                                                    |
| ------------------------ | --------------------------------------------------------- | ------------------------------------------------------------------ |
| `command-console-1.7.1/` | `dolusoft/command-console` `v1.7.1` (`6c85440`) yayın eki | `70f09fb4ed4f9d5b7e979ef18bdc1a2d5a98a12e24578967b4338a4e33268e44` |

Rust tarafında `crates/gecko-wire/tests/fixtures.rs` `valid`, `invalid`, `future`, `error-class.json` ve `command-name.json` örneklerini CLI'nın okuyucusuna karşı koşar. `sequence/` örnekleri satır yolunun süreç testleridir (adım 4b-5).
