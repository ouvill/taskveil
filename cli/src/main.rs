//! `taskveil` CLI。
//!
//! `taskveil-client`の共通client APIを通じてローカルの暗号化DBへ直接アクセスする設計だが
//! （`docs/03_技術仕様書.md` §8.1, §8.3）、DB統合前の現段階ではスタブとして
//! サブコマンドの受け口だけを提供し、operational commandは明示的にfail closedする。

use clap::{Parser, Subcommand};
use std::process::ExitCode;

// `taskveil-client`をfrontend共通入口としてcompile時にも固定する。実際の
// local profile openとsubcommand接続はOS secret store実装後の後続taskで行う。
use taskveil_client::{ClientError, LocalProfileConfig, TaskveilClient};

const _: fn(LocalProfileConfig) -> Result<TaskveilClient, ClientError> = TaskveilClient::open;

#[allow(dead_code)]
fn _assert_async_client_api(client: &TaskveilClient) {
    std::mem::drop(client.sync_now());
}

#[derive(Parser)]
#[command(
    name = "taskveil",
    version,
    about = "Taskveil: E2EE Todo CLI (operational commands unavailable in this build)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 新規タスクを追加する。
    Add { title: String },
    /// タスク一覧を表示する。
    List,
    /// タスクを完了状態にする。
    Done { id: String },
}

const UNAVAILABLE_DIAGNOSTIC: &str = "taskveil: operational commands are unavailable in this build";

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Add { title: _ } | Command::List | Command::Done { id: _ } => {}
    }
    eprintln!("{UNAVAILABLE_DIAGNOSTIC}");
    ExitCode::FAILURE
}
