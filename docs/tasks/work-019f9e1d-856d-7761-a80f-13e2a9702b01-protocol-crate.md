---
id: 019f9e1d-856d-7761-a80f-13e2a9702b01
title: Protocol-only wire contract crate
status: active
lane: standard
milestone: maintenance
---

## 1. 背景とコンテキスト

GitHub Issue #60で、serverのproduction dependencyが同期client runtimeを含む
`taskveil-sync`へ到達しているcrate境界違反が確認された。同期、account、
Organizationのwire contractを独立させ、serverがclient orchestrationへ依存しない
技術仕様の境界をCargo graphでも強制する。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md` §2、§6、§7
- `docs/tasks/task-86-protocol-v2-cas.md`
- `core/sync/src/{protocol,account,organization,key_manifest,envelope}.rs`
- `server/src/{sync,auth,organization}.rs`
- `.github/workflows/ci.yml`

## 3. ゴール

- serde DTO、wire enum、protocol/envelope version、wire上限だけを持つ
  `taskveil-protocol`をworkspaceへ追加する。
- serverのproduction dependencyから`taskveil-sync`を除去し、
  `taskveil-protocol`と必要最小限の`taskveil-crypto`だけを共有する。
- client syncはprotocol、domain、cryptoを明示的に組み合わせる。
- Cargo manifest boundaryをlocal gateとCIで継続検査する。

## 4. スコープ

### やること

- sync、account、Organizationの共有wire DTOをprotocol crateへ移す。
- sync側の既存public APIを必要なre-exportで移行する。
- serverが必要とするmanifest/envelope構造検証をcrypto境界へ移す。
- JSON shape、strict deserialize、version/header定数の回帰testを維持・強化する。
- protocol crateとserver production dependencyのmanifest boundary testを追加する。
- `docs/03_技術仕様書.md`のcrate構成とserver依存境界を外科的に更新する。

### やらないこと

- wire JSON shape、sync protocol version、envelope versionの変更。
- 暗号suite、鍵階層、同期state machine、DB schemaの変更。
- client orchestration、HTTP transport、暗号演算のprotocol crateへの移動。
- `docs/01_企画書.md` / `docs/02_機能仕様書.md`の変更。

## 5. 実装手順

1. 現在のwire DTOとserverが参照する暗号検証surfaceを分類する。
2. protocol-only crateを追加してDTO/constantsを移し、互換re-exportを整える。
3. server production dependencyをprotocol/cryptoへ限定する。
4. manifest boundary gateとfixture testをCIへ接続する。
5. workspace、server、Flutter影響範囲を検証する。

## 6. 受け入れ基準

- [ ] `taskveil-protocol`がserde DTO、wire enum、version/constantsだけを保持する。
- [ ] serverの`[dependencies]`に`taskveil-sync`がなく、protocolと必要最小cryptoだけを共有する。
- [ ] client syncがprotocol、domain、cryptoへ明示的に依存する。
- [ ] protocol crateからstorage、HTTP、client runtimeへの依存をmanifest gateが拒否する。
- [ ] serverからsync production dependencyが再導入された場合にboundary gateが失敗する。
- [ ] 既存wire JSON互換性testが移行後も成功し、主要account/Organization DTOを含む。
- [ ] 全Rust品質ゲート、client/protocol boundary gate、影響するFlutter gate、`git diff --check`が成功する。

## 7. 制約・注意事項

- protocol crateには暗号演算、HTTP client、storage、domain merge、client runtimeを置かない。
- release前方針によりRustのsource APIは整理してよいが、現行wire JSON shapeは維持する。
- serverはE2EE plaintextを解釈しない。
- 独立検証が完了するまで`status: active`を維持する。

## 8. 完了報告に含めるべき内容

- 抽出したDTO/constantsと残したruntime/crypto責務。
- server Cargo dependency graphの証拠。
- wire互換性test、boundary gate、全品質ゲートの結果。
- 移行re-exportと残る懸念。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果: `taskveil-protocol`をworkspaceへ追加し、sync/account/Organizationの共有wire DTO、enum、version/header、envelope framing、canonical HLC wire valueを抽出した。key manifestのMAC・Organization署名検証は`taskveil-crypto`へ移し、`taskveil-sync`にはHTTP transport、暗号record、merge、同期state machineと互換re-exportだけを残した。server productionは`taskveil-protocol` / `taskveil-crypto`だけを共有し、`taskveil-sync`をdev-only integration dependencyへ移した。
- 証拠: `cargo tree -p taskveil-server --edges normal -i taskveil-sync`は`nothing to print`、depth 1のTaskveil依存は`taskveil-crypto` / `taskveil-protocol`のみ。protocol wire 5 tests、Rust workspace 414 passed / 3 ignored、server Postgres統合19 tests、bridge release build、`flutter analyze`、`flutter test` 301 passed / visual QA 1 skipped、hardcoded-string/client/protocol boundary gate、protocol boundary negative fixtures、`git diff --check`が成功した。
- Commit: 未コミット
- 未解決: 独立検証未実施。既存のmacOS Keychain実機test 2件、10k storage性能test 1件、Flutter visual QA harness 1件は明示的な手動gateのためskip / ignored。

### 独立検証

- 判定: 未判定
- 根拠: 未実施
- 検証者: 未割当
