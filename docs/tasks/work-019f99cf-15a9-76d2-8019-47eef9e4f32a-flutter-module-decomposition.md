---
id: 019f99cf-15a9-76d2-8019-47eef9e4f32a
title: Flutter task UI module decomposition
status: done
lane: standard
milestone: maintenance
---

## 1. 背景とコンテキスト

`app/lib/src/screens/tasks_screen.dart` と
`app/lib/src/ui/task_components.dart` は、それぞれ約2,500行に達している。
画面状態、Home section、並べ替え、作成sheet、metadata、task row、完了animationが
少数のDartファイルへ集中しており、局所変更時の確認範囲が広い。

既存実装はwidget test、semantics、`ValueKey`、FRB DTOとの接続を通じて広く検証されて
いるため、この作業では責務分割だけを行い、UIとbehaviorの変更を持ち込まない。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `app/lib/src/screens/tasks_screen.dart`
- `app/lib/src/ui/task_components.dart`
- `app/test/widget_test.dart`
- `app/test/list_completion_motion_test.dart`
- `app/test/calendar_screen_test.dart`
- `app/test/performance_large_data_test.dart`
- `app/test/visual_qa/visual_qa_screenshots_test.dart`

## 3. ゴール

- task screenの画面状態、表示補助、Home section、action、reorderを責務別ファイルへ分ける。
- task共通UIのcapture、metadata、row、完了animation、priority表示を責務別ファイルへ分ける。
- 既存library URI、公開symbol、private namespaceを維持し、呼び出し側の変更を不要にする。
- 分割後も既存のFlutter品質ゲートを通せる状態にする。

## 4. スコープ

### やること

- Dart `part` / `part of` を用いた機械的な責務分割。
- 元の `tasks_screen.dart` と `task_components.dart` をlibrary入口として維持する。
- 分割後のformat、静的解析、Flutter test、UI文字列検査を実行する。

### やらないこと

- UI、navigation、state transition、animation timing、semanticsの変更。
- 公開class、function、typedef、enumの改名またはsignature変更。
- `ValueKey`、tooltip、表示文字列、テスト期待値の変更。
- ARB、Rust API、FRB生成物、新規依存の変更。
- private symbolのpublic化や独立libraryへの移行。

## 5. 実装手順

1. `tasks_screen.dart` のimportと公開 `TasksScreen` を入口に残し、`_TasksBody`、
   row shell、Home section、action、reorderをpartへ移す。
2. `task_components.dart` のimportを入口に残し、capture、metadata、task row、
   completion animation、priority dotをpartへ移す。
3. 元ファイルから各宣言を一度だけ移動し、宣言内容と順序依存のないlibrary semanticsを
   維持する。
4. Dart formatterとFlutter品質ゲートを実行する。
5. 実装結果と検証事実を完了報告へ記録する。

## 6. 受け入れ基準

- [x] 既存の `package:taskveil/src/screens/tasks_screen.dart` importがそのまま動作する。
- [x] 既存の `package:taskveil/src/ui/task_components.dart` importがそのまま動作する。
- [x] 公開symbol、private symbol、`ValueKey`、semantics、UI挙動に意図した差分がない。
- [x] ARB、Rust/FRB APIと生成物、テスト期待値、新規依存に差分がない。
- [x] `dart format --output=none --set-exit-if-changed` が対象Dartファイルで成功する。
- [x] `cd app && flutter analyze` が成功する。
- [x] bridge release build後の `cd app && flutter test` が成功する。
- [x] `sh app/tool/check_hardcoded_strings.sh` が成功する。
- [x] `git diff --check` が成功する。

## 7. 制約・注意事項

- `part` を使って既存libraryのprivate namespaceと型identityを維持する。
- `_TasksBodyState` はprivate fieldとmethodの相互依存が大きいため、初回はclass単位で移す。
- 分割先partにはimportを追加せず、入口libraryのimportを共有する。
- 既存のsource importを分割先へ直接変更しない。
- visual差分を伴わないためgolden更新は行わない。

## 8. 完了報告に含めるべき内容

- 作成したpartと各責務。
- 元ファイルと分割後ファイルの行数。
- 実行したformat、analyze、test、hardcoded strings、diff checkの結果。
- skip、実行不能、環境制約、未解決事項。
- 未コミットであること。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果:
  - `tasks_screen.dart` を270行のentryと5つのpartへ分割した。
  - `task_components.dart` を21行のentryと6つのpartへ分割した。
  - 合計11のpartへ責務を分け、既存library URI、private namespace、公開symbol、
    `ValueKey`、semantics、UI挙動を維持した。
  - 新規依存、ARB、Rust API、FRB生成物、テスト期待値に差分はない。
- 証拠:
  - 対象Dartファイルのformat成功。
  - `flutter analyze`: No issues。
  - bridge release build成功。
  - `flutter test`: 280件成功、visual QA harness 1件skip。
  - `sh app/tool/check_hardcoded_strings.sh`: 成功。
  - `git diff --check`: 成功。
- Commit: 7b381de
- 未解決: なし

### 独立検証

- 判定: 合格
- 根拠:
  - 分割前の宣言本文と、partを元の宣言順へ再構成した本文を空行を除いて比較し、
    `tasks_screen` は
    `e92f6eedb330c8f99def429f41eca432b75e58bb3c7f491b70d103bffaa2642d`、
    `task_components` は
    `4a5795ae663d33557dd524844dd56bfdeab46c89c27e6d7ef5a1694e406f0e69`
    でそれぞれSHA-256が一致した。宣言欠落・重複、公開/private symbol、
    `ValueKey`、semantics、animation timing、UI behaviorのコード差分はない。
  - entry側の既存importとlibrary URIを維持し、全partの
    `part of '../tasks_screen.dart'` / `part of '../task_components.dart'`
    対応を確認した。`flutter analyze` も `No issues found` であり、
    part graphとprivate namespaceは整合している。
  - tracked差分とuntracked一覧を確認し、変更は2つのentry、11のpart、
    このwork itemだけだった。ARB、Rust、FRB生成物、依存定義、
    テストファイル・期待値には差分がない。
  - 対象13 Dartファイルの
    `dart format --output=none --set-exit-if-changed`: 成功（変更0）。
  - `flutter analyze`: 成功（No issues）。
  - `cd app/rust && env CARGO_TARGET_DIR=target cargo build --release`: 成功。
  - `flutter test`: 280件成功。visual QA screenshot harness 1件は
    `TASKVEIL_VISUAL_QA=1` 未指定時の既定動作としてskip。
  - `sh app/tool/check_hardcoded_strings.sh`: 成功。
  - `git diff --check`: 成功。untrackedの11 partとwork itemも
    `git diff --no-index --check /dev/null <file>` で個別に成功。
  - 初回のsandbox内実行はFlutter SDK cacheへの書き込み拒否で停止したが、
    権限付き再実行では同じformat/analyze/checkが成功しており、コード失敗ではない。
- 検証者: 実装を担当していない独立レビューエージェント
