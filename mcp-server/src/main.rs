//! `taskveil-mcp-server`: LLMエージェント向けTodo操作インターフェース。
//!
//! TODO: rmcp SDK による stdio トランスポートの実装は後続タスク
//! （`docs/03_技術仕様書.md` §8.2）。本バイナリは現時点ではfail-closed stub。

// CLIと同じく、Taskveilのapplication serviceへはこの共通入口だけを使う。
use std::process::ExitCode;
use taskveil_client::{ClientError, LocalProfileConfig, TaskveilClient};

const _: fn(LocalProfileConfig) -> Result<TaskveilClient, ClientError> = TaskveilClient::open;

#[allow(dead_code)]
fn _assert_async_client_api(client: &TaskveilClient) {
    std::mem::drop(client.sync_now());
}

fn main() -> ExitCode {
    eprintln!("taskveil-mcp-server: MCP transport is unavailable in this build");
    ExitCode::FAILURE
}
