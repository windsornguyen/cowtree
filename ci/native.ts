import { resolve } from "node:path";
import { action, type ScriptExec } from "@dedalus-labs/hollywood/action-runtime";

export async function checkFilesystems(exec: ScriptExec, path: string | undefined): Promise<void> {
  if (!path) throw new Error("PATH is required for the native filesystem checks");
  await exec("sudo", ["apt-get", "update"]);
  await exec("sudo", ["apt-get", "install", "-y", "btrfs-progs", "xfsprogs"]);
  await exec("rustup", ["toolchain", "install", "1.97.1", "--profile", "minimal"]);
  await exec("cargo", ["+1.97.1", "build", "--locked", "-p", "cowtree-metadata", "--features", "fault-injection"]);
  const binary = resolve("target/debug/cowtree-metadata");
  await exec("sudo", ["env", `PATH=${path}`, `COWTREE_METADATA_BINARY=${binary}`, "bash", "scripts/fs_matrix.sh"]);
}

export const nativeFilesystems = action({
  name: "Native filesystem matrix",
  description: "Run native reflink support, refusal, and managed-workspace checks",
  localActionPath: "ci/native-filesystems",
  inputs: {},
  outputs: {},
  run: async ({ exec }) => {
    await checkFilesystems(exec, process.env.PATH);
    return {};
  },
});
