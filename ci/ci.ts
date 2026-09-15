import { always, defineMatrix, expr, job, workflow } from "@dedalus-labs/hollywood";
import { checkout, run, setupNode, uv } from "./steps.ts";
import { nativeFilesystems } from "./native.ts";
import { uses } from "@dedalus-labs/hollywood";

const python = defineMatrix({
  os: ["ubuntu-latest", "macos-latest"],
  "python-version": ["3.10", "3.11", "3.12", "3.13", "3.14"],
});
const setupUv = { uses: uv.uses } as const;

export const ci = workflow(
  {
    name: "CI",
    on: { push: { branches: ["main"] }, pull_request: { branches: ["main"] } },
    permissions: { contents: "read" },
    jobs: {
      workflows: job({
        name: "Generated workflows",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 10,
        steps: [
          checkout,
          setupNode,
          run("Install workflow dependencies", "npm", ["ci", "--ignore-scripts"]),
          run("Check typed workflows and generated files", "npm", ["run", "ci", "check"]),
        ],
      }),
      lint: job({
        name: "Lint",
        "runs-on": "ubuntu-latest",
        steps: [
          checkout,
          uv,
          run("Install Python", "uv", ["python", "install", "3.10"]),
          run("Install dependencies", "uv", [
            "sync",
            "--locked",
            "--group",
            "lint",
            "--group",
            "bench",
          ]),
          run("Ruff lint", "uv", ["run", "ruff", "check", "."]),
          run("Ruff format", "uv", ["run", "ruff", "format", "--check", "."]),
          run("Ty typecheck", "uv", ["run", "ty", "check"]),
        ],
      }),
      test: job({
        name: "Test",
        "runs-on": python.os,
        strategy: { "fail-fast": false, matrix: python },
        steps: [
          checkout,
          uv,
          run("Install Python", "uv", ["python", "install", python["python-version"]]),
          run("Install dependencies", "uv", ["sync", "--locked", "--group", "test"]),
          run("Run tests", "uv", ["run", "pytest"]),
        ],
      }),
      build: job({
        name: "Build",
        "runs-on": "ubuntu-latest",
        steps: [
          checkout,
          uv,
          run("Install Python", "uv", ["python", "install", "3.10"]),
          run("Build package", "uv", ["build"]),
        ],
      }),
      filesystems: job({
        name: "Native filesystem matrix",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 15,
        steps: [checkout, setupUv, uses(nativeFilesystems, { with: {} })],
      }),
      specification: job({
        name: "Workspace protocol model",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 15,
        steps: [
          checkout,
          setupUv,
          {
            uses: "actions/setup-java@de7274f081f381c8f8158605e0321c36c376e2e6",
            with: { distribution: "temurin", "java-version": "17" },
          },
          run("Check safety, witnesses, and fencing mutation", "uv", [
            "run",
            "--locked",
            "--no-default-groups",
            "python",
            "scripts/check_specs.py",
            "--cache",
            expr<string>("format('{0}/cowtree-tla', runner.temp)"),
          ]),
          {
            name: "Save checker evidence",
            if: always(),
            uses: "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            with: {
              name: "workspace-protocol-evidence",
              "if-no-files-found": "error",
              path: ["java-version.txt", "*/tlc.log", "*/*.tla", "*/*.cfg"]
                .map((path) => `\u0024{{ runner.temp }}/cowtree-tla/${path}`)
                .join("\n"),
            },
          },
        ],
      }),
    },
  },
  { filename: "ci.yml" },
);
