import { always, expr, job, uses, workflow } from "@dedalus-labs/hollywood";
import { readFileSync } from "node:fs";
import { checkout, run, rust, setupNode } from "./steps.ts";
import { nativeFilesystems } from "./native.ts";

const atlasRevision = readFileSync("tools/atlas-revision.txt", "utf8").trim();
if (!/^[a-f0-9]{40}$/.test(atlasRevision)) throw new Error("Atlas revision must be a full Git commit");
const atlasPath = expr<string>("format('{0}/.tools/atlas', github.workspace)");

export const ci = workflow(
  {
    name: "CI",
    on: { push: { branches: ["main"] }, pull_request: {}, workflow_dispatch: {} },
    permissions: { contents: "read" },
    env: { RUSTUP_TOOLCHAIN: "1.97.1" },
    jobs: {
      workflows: job({
        name: "Generated workflows",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 10,
        steps: [checkout, setupNode,
          run("Install workflow dependencies", "npm", ["ci", "--ignore-scripts"]),
          run("Check typed workflows and generated files", "npm", ["run", "ci", "check"]),
        ],
      }),
      schema: job({
        name: "Declarative schema",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 15,
        env: { COWTREE_ATLAS: atlasPath, GOTOOLCHAIN: "local", ATLAS_NO_UPDATE_NOTIFIER: "1", ATLAS_NO_UPGRADE_SUGGESTIONS: "1" },
        steps: [
          checkout,
          { ...checkout, name: "Check out Atlas Community source", with: { repository: "ariga/atlas", ref: atlasRevision, path: ".tools/atlas-source", "persist-credentials": false } },
          { uses: "actions/setup-go@924ae3a1cded613372ab5595356fb5720e22ba16", with: { "go-version": "1.26.5", "cache-dependency-path": ".tools/atlas-source/**/go.sum" } },
          { ...run("Build Atlas Community", "go", ["build", "-mod=readonly", "-trimpath", "-o", atlasPath, "."]), "working-directory": ".tools/atlas-source/cmd/atlas" },
          rust,
          run("Check generated SQLite schema", "cargo", ["run", "--locked", "-p", "xtask", "--", "schema", "check", "--atlas", ".tools/atlas"]),
          run("Test declarative schema admission", "cargo", ["test", "--locked", "-p", "xtask", "schema"]),
        ],
      }),
      build: job({
        name: "Build",
        "runs-on": "ubuntu-24.04",
        steps: [checkout, rust, run("Build native executables", "cargo", ["build", "--locked", "--release", "-p", "cowtree-cli", "-p", "cowtree-metadata"])],
      }),
      filesystems: job({
        name: "Native filesystem matrix",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 20,
        steps: [checkout, uses(nativeFilesystems, { with: {} })],
      }),
      windows: job({
        name: "Windows ReFS native cloning",
        "runs-on": "windows-2025",
        "timeout-minutes": 15,
        steps: [checkout, rust, run("Check ReFS isolation and NTFS refusal", "pwsh", ["-NoProfile", "-File", "scripts/test_windows.ps1"])],
      }),
      "windows-arm": job({
        name: "Windows ARM64 Dev Drive cloning",
        "runs-on": "windows-11-arm",
        "timeout-minutes": 15,
        env: { RUSTUP_TOOLCHAIN: "1.97.1-aarch64-pc-windows-msvc" },
        steps: [checkout, rust, run("Check Dev Drive isolation and NTFS refusal", "pwsh", ["-NoProfile", "-File", "scripts/test_windows.ps1", "-DevDrive"])],
      }),
      specification: job({
        name: "Workspace protocol model",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 15,
        steps: [
          checkout, rust,
          { uses: "actions/setup-java@de7274f081f381c8f8158605e0321c36c376e2e6", with: { distribution: "temurin", "java-version": "17" } },
          run("Check safety, witnesses, and fencing mutation", "cargo", ["run", "--locked", "-p", "xtask", "--", "specs", "--cache", expr<string>("format('{0}/cowtree-tla', runner.temp)")]),
          { name: "Save checker evidence", if: always(), uses: "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a", with: {
            name: "workspace-protocol-evidence", "if-no-files-found": "error",
            path: ["java-version.txt", "*/tlc.log", "*/counterexample.json", "*/*.tla", "*/*.cfg"].map((path) => `\u0024{{ runner.temp }}/cowtree-tla/${path}`).join("\n"),
          } },
        ],
      }),
    },
  },
  { filename: "ci.yml" },
);
