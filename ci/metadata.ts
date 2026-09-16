import { defineMatrix, expr, format, job, workflow } from "@dedalus-labs/hollywood";
import { checkout, run, uv } from "./steps.ts";

const platforms = defineMatrix({ os: ["ubuntu-24.04", "macos-latest"] });
export const metadata = workflow(
  {
    name: "SQLite metadata",
    on: { push: { branches: ["main"] }, pull_request: { branches: ["main"] } },
    permissions: { contents: "read" },
    jobs: {
      metadata: job({
        name: format("SQLite ({0})", platforms.os),
        "runs-on": platforms.os,
        "timeout-minutes": 15,
        strategy: { "fail-fast": false, matrix: platforms },
        steps: [
          checkout,
          run("Install Rust", "rustup", [
            "toolchain",
            "install",
            "1.97.1",
            "--profile",
            "minimal",
            "--component",
            "rustfmt",
            "--component",
            "clippy",
          ]),
          run("Format", "cargo", ["+1.97.1", "fmt", "--all", "--check"]),
          run("Lint", "cargo", [
            "+1.97.1",
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
          ]),
          {
            ...run("Update Linux package index", "sudo", ["apt-get", "update"]),
            if: expr<boolean>("runner.os == 'Linux'"),
          },
          {
            ...run("Install Linux reflink test filesystem", "sudo", [
              "apt-get", "install", "-y", "btrfs-progs",
            ]),
            if: expr<boolean>("runner.os == 'Linux'"),
          },
          run("Test crashes and concurrent publication on native reflinks", "bash", ["scripts/metadata_test.sh"]),
          { ...uv, if: expr<boolean>("runner.os == 'macOS'") },
          {
            ...run("Test managed Python workspaces on APFS", "uv", [
              "run", "--locked", "--no-default-groups", "--group", "test",
              "pytest", "tests/test_workspace.py", "-q",
            ]),
            if: expr<boolean>("runner.os == 'macOS'"),
            env: {
              COWTREE_METADATA_BINARY: expr<string>("format('{0}/target/debug/cowtree-metadata', github.workspace)"),
              COWTREE_EXPECT_SUPPORTED: "1",
            },
          },
          {
            ...run("Run production example with test hooks disabled", "cargo", [
              "+1.97.1",
              "run",
              "--locked",
              "-p",
              "cowtree-metadata",
              "--example",
              "publish",
            ]),
            env: { COWTREE_CRASH_AT: "after-sql-commit", COWTREE_PAUSE_AT: "after-upload-pin" },
          },
          {
            ...run("Check documentation", "cargo", [
              "+1.97.1",
              "doc",
              "--locked",
              "--workspace",
              "--no-deps",
            ]),
            env: { RUSTDOCFLAGS: "-D warnings" },
          },
        ],
      }),
      "minimum-rust": job({
        name: "SQLite minimum Rust",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 15,
        steps: [
          checkout,
          run("Install minimum Rust", "rustup", [
            "toolchain",
            "install",
            "1.85.0",
            "--profile",
            "minimal",
          ]),
          run("Check minimum supported compiler", "cargo", [
            "+1.85.0",
            "check",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
          ]),
        ],
      }),
    },
  },
  { filename: "metadata.yml" },
);
