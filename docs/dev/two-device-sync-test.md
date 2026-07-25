# 2台同期テスト手順

この手順は、ローカルPostgres + `taskveil-server` を使って2つのTaskveilクライアント間の登録、ログイン、同期を手動確認するための開発用メモである。

## 1. 開発サーバーを起動する

リポジトリルートで実行する。

```sh
./tool/dev_server.sh
```

スクリプトは `taskveil-dev-postgres` コンテナを再利用または作成し、`server/migrations/*.sql` を適用してから `cargo run -p taskveil-server` を `http://localhost:8080` で起動する。Postgresのホスト側ポートは、5432が埋まっている場合に5433以降の空きポートへ自動でずらす。

別ターミナルでヘルスチェックする。

```sh
curl -i http://localhost:8080/health
```

`HTTP/1.1 200 OK` と `{"status":"ok"}` が返ればよい。

停止はサーバーターミナルで `Ctrl-C`。DBを止める場合は次を実行する。

```sh
docker stop taskveil-dev-postgres
```

DBを作り直す場合は次を実行する。

```sh
docker rm -f taskveil-dev-postgres
```

## 2. クライアントを2台起動する

### iOSシミュレータ2台

利用可能なシミュレータを確認する。

```sh
xcrun simctl list devices available
```

例としてiPhone 17とiPhone 17 Proを起動する。実際のデバイス名は手元の一覧に合わせる。

```sh
xcrun simctl boot "iPhone 17" || true
xcrun simctl boot "iPhone 17 Pro" || true
open -a Simulator
```

Flutterから見えるdevice idを確認する。

```sh
cd app
flutter devices
```

2つのターミナルで、それぞれ別device idを指定して起動する。

```sh
cd app
flutter run -d <SIMULATOR_DEVICE_ID_1>
```

```sh
cd app
flutter run -d <SIMULATOR_DEVICE_ID_2>
```

### iOSシミュレータ + macOSアプリ

片方をiOSシミュレータで起動する。

```sh
cd app
flutter run -d <SIMULATOR_DEVICE_ID>
```

もう片方をmacOSで起動する。

```sh
cd app
flutter run -d macos
```

### Android emulatorを使う場合

credential issuerはHTTPSを必須とし、HTTPはloopbackだけを許可する。Android emulatorからホストのlocal serverへ接続する場合は、先にADB reverseを設定する。

```sh
adb -s <ANDROID_EMULATOR_DEVICE_ID> reverse tcp:8080 tcp:8080
```

そのうえでAndroid、iOS Simulator、macOSのすべてに `http://127.0.0.1:8080` を設定する。Androidのhost alias `http://10.0.2.2:8080` はloopback originではないため、認証credentialのissuerには使用しない。

## 3. アカウント画面でサーバーURLを設定する

各クライアントでアカウント画面を開き、Server URLに次を入力して保存する。

```text
http://127.0.0.1:8080
```

## 4. 1台目で登録する

1台目のアカウント画面でRegisterを選び、開発用のメールアドレスとパスワードで登録する。

例:

```text
sync-dev@example.com
```

登録後、Recovery Keyが表示される場合はこの手動テスト中だけ控える。実運用では秘密情報として扱い、ログやスクリーンショットへ残さない。

## 5. 2台目でログインする

2台目のアカウント画面でLog inを選び、1台目と同じメールアドレス、パスワード、Server URLでログインする。

ログインできたら、アカウント画面の同期状態が待機中または同期済みになることを確認する。必要なら両端末で「今すぐ同期」を押す。

## 6. 同期を確認する

1. 1台目でリストまたはタスクを作成する。
2. 1台目のアカウント画面で「今すぐ同期」を押す。
3. 2台目のアカウント画面で「今すぐ同期」を押す。
4. 2台目のタスク一覧に、1台目で作成した内容が表示されることを確認する。
5. 2台目で同じタスクのタイトル、メモ、完了状態などを変更する。
6. 2台目、1台目の順に「今すぐ同期」を押し、1台目へ変更が戻ることを確認する。
7. 片方をオフライン相当にして編集したい場合は、サーバーを一度 `Ctrl-C` で止めて編集し、再度 `./tool/dev_server.sh` で起動してから「今すぐ同期」を押す。

期待結果:

- 登録とログインが成功する。
- 片方で作成したタスクがもう片方に現れる。
- 双方向の編集が最終的に同じ状態へ収束する。
- 削除したタスクが他方で復活しない。
- 同期失敗時に秘密情報、パスワード、Device Key、Master Key、Recovery Keyがログへ出ない。

この手順のlocal serverはrealtime環境変数なしでは通知disabledで起動するため、上記だけでWebSocket即時性を確認済みとは扱わない。production credentialを使わない自動統合確認はrepo rootから次を実行する。

```sh
cargo test -p taskveil-server realtime --lib
cd realtime-worker && npm test
cd app && flutter test test/realtime_integration_test.dart
```

Worker testはlocal Durable Object上の2接続、送信元除外、duplicate、expiry、hibernation evictionを確認する。Flutter testはfake clock / socketでmutation commitからremote sync開始まで500ms、socket停止中のlocal sync、30秒fallback、resume syncを確認する。

deployed Workerとの確認は、staging相当環境で両端末をforegroundにして片方のmutationが30秒poll周期より前に他方のsyncを起動することを観測する。続いて通知providerを安全に停止できる検証環境で、local mutationとHTTPS syncが成功し、30秒fallbackまたはforeground復帰で収束することを確認する。実Cloudflare latency、EU jurisdiction、hibernation、費用はこの人間確認で記録し、public logやスクリーンショットへcredential、ticket、opaque identifierを残さない。

## 7. よく使う確認コマンド

Postgresコンテナの状態:

```sh
docker ps --filter name=taskveil-dev-postgres
```

サーバーヘルスチェック:

```sh
curl -s http://localhost:8080/health
```

8080を使っているプロセス:

```sh
lsof -nP -iTCP:8080 -sTCP:LISTEN
```
