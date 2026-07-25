---
id: 019f9a15-77c3-72d2-9fdd-c865a6089769
title: FRB contract coverage and private conversion extraction
status: done
lane: standard
milestone: maintenance
---

# FRB contract coverage and private conversion extraction

## 1. 背景とコンテキスト

`app/rust/src/api.rs` は公開FRB DTO・関数と、privateな入力parse・DTO変換・JSON
encoding helperを同居させている。また、公開関数のsignatureを固定する既存testは、
73関数のうち23関数を検査していない。

公開Dart contractと `crate::api` pathを維持したまま、先にsignature safety netを
完全化し、その後private helperだけを別moduleへ移してbridge sourceをレビューしやすくする。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md` §1.3
- `docs/dev/client-profile-architecture.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `app/rust/src/api.rs`
- `app/rust/src/client_handle.rs`
- `flutter_rust_bridge.yaml`

## 3. ゴール

- 既存73公開FRB関数すべてのRust signatureをcompile-time testで固定する。
- privateなparse・DTO変換・JSON helperを `app/rust/src/api/conversions.rs` へ集約する。
- 公開API、Dart call surface、実行時挙動を変更しない。抽出に伴うcodegen metadataと
  同一toolchainが生成する整形差分だけは記録した上で許容する。

## 4. スコープ

### やること

- `all_public_function_signatures_remain_stable` に不足している23関数を追加する。
- `client_result` から `json_string` までのprivate helperを子moduleへ純粋移動する。
- parent moduleからだけ参照できる最小限のvisibilityとimportを設定する。
- FRB再生成後に公開Dart宣言・signatureのsemantic diffがないことを確認する。
- 生成物をcommitした後にcodegenを再実行し、worktreeがcleanなことを確認する。

### やらないこと

- 公開DTO・公開関数・`crate::api` pathの移動、rename、signature変更。
- FRB生成物の手編集。
- `TaskveilClient`、domain、storage、sync、Flutterの変更。
- helperの挙動変更、整理、追加リファクタリング。
- 新規依存追加。

## 5. 実装手順

1. 現行73公開関数とsignature testの差を固定する。
2. 不足する同期・async signature assertionを追加する。
3. private helperを `api/conversions.rs` へ純粋移動する。
4. format、clippy、test、boundary、FRB codegenと生成物diffを実行する。
5. 実装結果と検証事実を完了報告へ記録する。

## 6. 受け入れ基準

- [x] 既存73公開関数すべてをsignature contract testが参照している。
- [x] 公開DTO・公開関数は `app/rust/src/api.rs` に残っている。
- [x] private helperだけが `app/rust/src/api/conversions.rs` へ移動している。
- [x] `crate::api` pathとDart call surfaceが不変である。
- [x] FRB再生成後に公開Dart宣言・signatureのsemantic diffがない。
- [x] 生成差分はprivate helperのignored metadataと同一toolchainの整形だけに限定される。
- [x] 生成物をcommitした後の再codegenでworktreeがcleanになる。
- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy -p taskveil_app_bridge --all-targets -- -D warnings`
- [x] `cargo test -p taskveil_app_bridge`
- [x] `cargo check --workspace --all-targets`
- [x] `sh app/tool/check_client_boundaries.sh`
- [x] `sh app/tool/test_client_boundaries.sh`
- [x] `git diff --check`

## 7. 制約・注意事項

- `rust_input: crate::api` を維持し、codegenが読む公開surfaceを子moduleへ移さない。
- `conversions.rs` はhandwritten bridge codeなのでclient boundary checkの対象とする。
- private helperのignored metadata削除と同一FRB 2.12.0による決定的整形差分は、
  公開surface不変を機械確認して完了報告へ記録する。その他の生成surface差分が出た場合は
  停止して原因を解消する。
- 実装者は独立検証の合否を記入せず、front matterを `done` にしない。

## 8. 完了報告に含めるべき内容

- contract testで追加した関数群と全73件を確認した方法。
- 移動したhelper範囲とbefore / after行数。
- FRB生成物のsemantic diff確認と、commit後の再生成clean確認。
- 実行した品質ゲートと結果。
- Commit hash、未解決事項、環境制約。

## 9. 完了報告

### 実装結果

- `all_public_function_signatures_remain_stable` に不足していた23関数を追加した。
  local time zone、device key rotation、organization safety number、sync outcome、billing、
  template、task series、settlement、streakの同期・async signatureを固定した。
- `app/rust/src/api.rs` の公開関数を列挙する機械確認とtest内参照との差分を取り、
  73公開関数すべてがcontract testから参照され、未参照が0件であることを確認した。
- `client_result` から `json_string` までのprivate helper 41関数だけを
  `app/rust/src/api/conversions.rs` へ純粋移動し、子module内では
  `pub(super)` に限定した。公開DTO・公開関数と `crate::api` pathは親moduleに残した。
- `api.rs` は1,459行から1,040行になり、抽出先は483行となった。抽出先の増分には
  module import、限定visibility、rustfmtによる折り返しを含む。
- FRB 2.12.0で再生成し、公開Dart宣言・signatureのsemantic diffがないことを確認した。
  `app/lib/src/rust/api.dart` の差分は、移動したprivate helper 41関数に対する
  ignored metadata commentの削除1行だけであり、生成結果として採用した。
- `app/lib/src/rust/api.freezed.dart` に生じた差分は空行末尾の空白10件だけだった。
  公開surfaceにも生成内容にも影響しない同一toolchainの整形差分であり、
  CIと同じ `--ignore-space-at-eol` で同値を確認した上で、ファイル単位では採用せず
  生成ファイル全体をrestoreした。生成物を手編集していない。
- 生成物を含む実装commit後にFRB codegenを再実行し、今回は行末空白を含めて差分がなく、
  worktreeがcleanであることを確認した。
- `cargo fmt --all -- --check`、app bridge clippy、app bridge test（3件）、
  workspace全target check、client boundary check / fixture test、`flutter analyze`、
  `git diff --check` はすべて成功した。
- sandbox内の初回FRB codegenとFlutter解析は共有Flutter SDK cacheへの書き込み制約で
  実行できなかったため、承認済みの外部実行で完了した。初回Flutter解析時は
  nested cargokit build toolの依存が未取得だったため、同directoryで
  `dart pub get` を実行してから再実行し、`No issues found` を確認した。
- Commit: この変更を含むcommit（hashはGit履歴を正本とする）。
- 未解決事項: 独立検証は未実施。実装者による完了判定とstatus変更は行わず、
  front matterは `active` のままとする。

### 独立検証

- 検証日: 2026-07-26
- 判定: 合格
- 対象Commit: `8836f6b0af131c354395fdc58d972f7f90c77f71`
- 契約確認: `api.rs` の公開関数73件と
  `all_public_function_signatures_remain_stable` のassertion 73件を機械照合し、
  未参照0件を確認した。実装前後の公開関数名、および公開struct / enum名にも
  差分はなく、公開DTO・公開関数は引き続き `api.rs` に置かれている。
- helper確認: 旧 `api.rs` の `client_result` から `json_string` までの41関数と、
  新しい `api/conversions.rs` の41関数を、`pub(super)` とrustfmtの折り返しを
  正規化して比較し、処理内容の差分がないことを確認した。新moduleのvisibilityは
  parent限定である。
- FRB / Dart確認: `flutter_rust_bridge_codegen 2.12.0` で実装commit後に再生成し、
  `git status --short` と生成物diffが空であることを確認した。実装commitの生成物差分は
  `app/lib/src/rust/api.dart` のprivate helper ignored metadata comment 1行の削除だけで、
  その他のRust / Dart / C生成物に差分はない。`flutter_rust_bridge.yaml` の
  `rust_input: crate::api` と公開Dart宣言・signatureは不変である。
- 品質ゲート: `cargo fmt --all -- --check`、
  `cargo clippy -p taskveil_app_bridge --all-targets -- -D warnings`、
  `cargo test -p taskveil_app_bridge`（3件成功）、
  `cargo check --workspace --all-targets`、`flutter analyze`（`No issues found`）、
  `sh app/tool/check_client_boundaries.sh`、
  `sh app/tool/test_client_boundaries.sh`、`git diff --check`が成功した。
- 環境条件: sandbox内のFRB codegenとFlutter解析はFlutter SDK cacheへの
  書き込み制約で失敗したため、承認済み外部実行で同じコマンドを再実行して成功した。
- 検証者: 実装を担当していない独立レビューエージェント
- 指摘・未解決事項: なし。
