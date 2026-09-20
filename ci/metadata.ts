import { defineMatrix, expr, format, job, workflow } from "@dedalus-labs/hollywood";
import { checkout, run, uv } from "./steps.ts";

const platforms = defineMatrix({ os: ["ubuntu-24.04", "macos-latest"] });
export const metadata = workflow(
  {
    name: "SQLite metadata",
    on: { push: { branches: ["main"] }, pull_request: {} },
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
          run("Test crashes and concurrent publication", "cargo", [
            "+1.97.1",
            "test",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
          ]),
          run("Build production metadata interface", "cargo", [
            "+1.97.1", "build", "--locked", "-p", "cowtree-metadata",
          ]),
          uv,
          run("Install Python integration dependencies", "uv", ["sync", "--locked", "--group", "test"]),
          run("Build native command interfaces", "cargo", [
            "+1.97.1", "build", "--locked", "--release", "-p", "cowtree-cli",
          ]),
          {
            ...run("Check native commands and Git aliases", "uv", [
              "run", "--no-sync", "pytest", "-q", "tests/test_cli.py", "tests/test_cli_json.py",
              "tests/test_existing_branch.py", "tests/test_committed_ref.py", "tests/test_git_extension.py",
            ]),
            env: {
              COWTREE_TEST_BINARY: expr<string>("format('{0}/target/release/cowtree', github.workspace)"),
            },
          },
          run("Test Python metadata integration", "uv", ["run", "--no-sync", "pytest", "-q", "integration"]),
          {
            ...run("Test APFS workspace integration", "uv", ["run", "--no-sync", "pytest", "-q", "mounted"]),
            if: expr<boolean>("runner.os == 'macOS'"),
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
