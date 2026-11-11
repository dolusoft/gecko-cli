# Fixtures

`command-console-<sürüm>/` klasörleri command-console sürümünün yayın ekinden (`remote-fixtures-<sürüm>.tar.gz`) **olduğu gibi** kopyalanır: `remote/` örnekleri ve `remote-contract.md` sözleşmesi. Elle düzenlenmez, biçimlendirilmez (`.prettierignore`). Yeni sürüm yeni klasörle gelir; protokol biçimi değişmediyse eski klasör kalkar (1.7.1 ve 1.7.2 yalnız örnek ekledi: önceki örnekler aynen içinde). 1.8.0 protokole alan ekledi (`effect`, `approval`, `--dry-run`); 1.7.2 klasörü ve testleri yerinde kalır. 1.8.2 yalnız örnek ekledi (`sequence/approval.json`'da süresi dolan kart: `change-server-card-expired`, `change-server-card-approved-late`; örneklerin `tab` nesnesine `expire`), sözleşme metni aynı; 1.8.0 klasörü kalktı.

| Klasör                   | Kaynak                                                    | Arşivin SHA-256                                                    |
| ------------------------ | --------------------------------------------------------- | ------------------------------------------------------------------ |
| `command-console-1.7.2/` | `dolusoft/command-console` `v1.7.2` (`d46c109`) yayın eki | `9049d979f041e15669baf60159ed14c7809f1f05a6909be338a45fce0b56c7c2` |
| `command-console-1.8.2/` | `dolusoft/command-console` `v1.8.2` (`719a5aa`) yayın eki | `05826e61cea4386b64db34c1fd0664890fcf2a5ae3aabbc6ec646ed57463073b` |

Rust tarafında `crates/testkit/tests/wire_fixtures.rs` her klasörün `valid`, `invalid`, `future`, `error-class.json` ve `command-name.json` örneklerini `gecko-wire` okuyucusuna karşı koşar. `crates/testkit/tests/sequence.rs` her klasörün `sequence/` örneklerini satır yolunun durum makinesine koşar; `crates/app/tests/e2e.rs` 1.8.2 `sequence/` örneklerini gerçek ikili ve gerçek geliştirme hub'ı üzerinden koşar.
