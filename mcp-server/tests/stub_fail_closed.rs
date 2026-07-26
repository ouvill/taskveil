use std::process::Command;

#[test]
fn stub_fails_without_writing_to_protocol_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_taskveil-mcp-server"))
        .output()
        .expect("run taskveil MCP stub");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"taskveil-mcp-server: MCP transport is unavailable in this build\n"
    );
}
