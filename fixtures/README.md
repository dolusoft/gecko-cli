# Fixtures

`command-console-<sürüm>/` klasörleri command-console sürümünün yayın ekinden (`remote-fixtures-<sürüm>.tar.gz`) **olduğu gibi** kopyalanır: `remote/` örnekleri ve `remote-contract.md` sözleşmesi. Elle düzenlenmez, biçimlendirilmez (`.prettierignore`). Yeni sürüm yeni klasörle gelir; protokol biçimi değişmediyse eski klasör kalkar (1.7.1 ve 1.7.2 yalnız örnek ekledi: önceki örnekler aynen içinde).

| Klasör                   | Kaynak                                                    | Arşivin SHA-256                                                    |
| ------------------------ | --------------------------------------------------------- | ------------------------------------------------------------------ |
| `command-console-1.7.2/` | `dolusoft/command-console` `v1.7.2` (`d46c109`) yayın eki | `9049d979f041e15669baf60159ed14c7809f1f05a6909be338a45fce0b56c7c2` |

Rust tarafında `crates/gecko-wire/tests/fixtures.rs` `valid`, `invalid`, `future`, `error-class.json` ve `command-name.json` örneklerini CLI'nın okuyucusuna karşı koşar. `sequence/` örnekleri satır yolunun süreç testleridir (adım 4b-5).
