import { defineMatrix, format, job, workflow } from "@dedalus-labs/hollywood";
import { checkout, run } from "./steps.ts";

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
          run("Build native command interfaces", "cargo", [
            "+1.97.1", "build", "--locked", "--release", "-p", "cowtree-cli",
          ]),
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
