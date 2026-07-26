---
id: 019f9c88-c3be-75b6-9521-eab3fb37df98
title: Fail closed for unavailable CLI and MCP frontends
status: active
lane: standard
milestone: maintenance
---

## 1. 背景とコンテキスト

未実装のCLI operational commandはstdoutへplaceholderを出してexit 0となり、
MCP stubもstdio protocol外の文字列をstdoutへ出してexit 0となる。automationや
MCP hostが未実装処理を成功と誤認しないよう、実接続の前提が揃うまでは明示的に
fail closedにする。

出典はpublic Issue #63である。共有profileへの実接続はIssue #55のprocess間
coordinationとproduction OS secret storeの完了後に別work itemで扱う。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/adr/ADR-011.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/BACKLOG.md`
- `cli/src/main.rs`
- `mcp-server/src/main.rs`
- `.github/workflows/ci.yml`

## 3. ゴール

- 未提供のCLI commandが副作用なしでexit 1となり、stdoutを汚さない。
- MCP stubがJSON-RPC successを装わず、stdoutを汚さずexit 1となる。
- debug testだけでなくrelease binaryでも同じ契約をCIで固定する。

## 4. スコープ

### やること

- CLI `add` / `list` / `done`の固定unsupported診断とfailure exit
- MCP stubの固定unsupported診断とfailure exit
- process integration test
- release binary regression scriptとCI gate
- packaging / shared-profile依存の公開文書更新

### やらないこと

- `TaskveilClient`によるprofile open、CRUD、検索、同期
- MCP transport、tool schema、JSON-RPC実装
- OS secret store、profile coordination、DB / protocol変更
- CLI / MCP artifact配布の新設

## 5. 実装手順

1. operational pathのstdout出力と入力echoを除去する。
2. stderrの固定diagnosticとportableなfailure exitを実装する。
3. help / version / clap parse errorを含むprocess testを追加する。
4. release binaryを実行するCI regression scriptを追加する。
5. BACKLOGの実接続依存をIssue #55 / #63へ合わせる。
6. 統合HEADで品質ゲートと独立レビューを行う。

## 6. 受け入れ基準

- [x] CLI `add` / `list` / `done`はexit 1、stdout empty、stderr固定診断となる。
- [x] CLIはtitle / idをstdoutまたはstderrへ出さない。
- [x] CLI `--help` / `--version`はexit 0、invalid argsは非0を維持する。
- [x] MCP stubはhangせずexit 1、stdout empty、stderr固定診断となる。
- [x] debug process testとrelease binary regression checkが成功する。
- [x] CLI / MCP manifestはpublish不可を明示する。
- [x] MCPからCLI binaryへの依存を追加しない。
- [x] workspace品質ゲートと独立検証が成功する。

## 7. 制約・注意事項

- 未実装を成功contentとして返さない。
- operational input、profile path、credentialをdiagnosticへ含めない。
- 実接続をIssue #55より先に有効化しない。
- 将来のartifact workflowはready markerを持つ明示allowlistとし、本work itemでは
  配布経路を新設しない。

## 8. 完了報告に含めるべき内容

- CLI / MCPのexit、stdout、stderr契約
- debug / release regression test結果
- packagingとIssue #55への依存
- 品質ゲート、独立検証、PR / CI / merge結果

## 9. 完了報告

### 実装結果

- CLI `add` / `list` / `done`は固定diagnosticをstderrへ出してexit 1となり、
  stdoutやtitle / idを出力しない。help / versionはexit 0、parse errorは非0を維持する。
- MCP stubはstdio protocolのstdoutへ何も書かず、固定diagnosticとexit 1で直ちに停止する。
- 両binaryのprocess integration testに加え、release buildを直接起動する
  `tool/ci/check_stub_frontends.sh`をRust quality jobへ追加した。
- CLI / MCP crateは`publish = false`を明示し、実接続はIssue #55のprocess間
  coordinationとproduction OS secret storeを完了する後続work itemまで有効化しない。
- 品質ゲート: fmt、workspace all-target clippy、workspace test
  （409 passed / 3既存manual ignored）、client boundary、release regression、
  shell syntax、diff-checkが成功した。sandbox内の初回workspace testはloopback bindを
  OS error 1で拒否されたため、通常のローカル権限で全件を再実行して成功した。

### 独立検証

- 判定: 承認（blockerなし）
- 根拠: CLI process 2件、MCP process 1件、release build / regression script、
  対象crate all-target clippy、fmt / diff-checkを独立再実行した。stdout、exit、
  固定stderr、入力非echo、help / version、parse error、CI配置、publish不可、
  Issue #55依存を確認した。
- 検証者: 独立レビュー担当 agent

### GitHub

- Commit: 本work itemと同じcommitで記録する。
- PR / CI / merge: 作成後に追記する。
