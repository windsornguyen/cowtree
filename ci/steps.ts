import { command, expr, type GitHubCommandStep } from "@dedalus-labs/hollywood";

export const checkout = { uses: "actions/checkout@v6" } as const;
export const uv = {
  uses: "astral-sh/setup-uv@bec219d24cd3e171d82865faccec33120bb574f4",
  with: { "enable-cache": true, "cache-dependency-glob": "uv.lock" },
} as const;
export const setupNode = {
  uses: "actions/setup-node@48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e",
  with: { "node-version": "24" },
} as const;
export function run(name: string, file: string, args: readonly string[]): GitHubCommandStep {
  return { name, run: command({ file, args }) };
}

export const rust = run("Install Rust", "rustup", [
  "toolchain", "install", expr<string>("env.RUSTUP_TOOLCHAIN"), "--profile", "minimal",
]);
