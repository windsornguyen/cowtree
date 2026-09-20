import { action, type ScriptExec } from "@dedalus-labs/hollywood/action-runtime";
import { dirname, isAbsolute } from "node:path";

export async function checkFilesystems(exec: ScriptExec, path: string | undefined): Promise<void> {
  if (!path) throw new Error("PATH is required for the native filesystem checks");
  await exec("sudo", ["apt-get", "update"]);
  await exec("sudo", ["apt-get", "install", "-y", "btrfs-progs", "xfsprogs"]);
  await exec("rustup", ["toolchain", "install", "1.97.1", "--profile", "minimal"]);
  await exec("cargo", ["+1.97.1", "build", "--locked", "-p", "cowtree-metadata"]);
  const selected = await exec("rustup", ["which", "--toolchain", "1.97.1", "cargo"]);
  const cargo = selected.stdout.trim();
  if (!isAbsolute(cargo)) throw new Error("rustup did not return an absolute Cargo executable");
  await exec("sudo", ["env", `PATH=${dirname(cargo)}:${path}`, "bash", "scripts/fs_matrix.sh"]);
}

export const nativeFilesystems = action({
  name: "Native filesystem matrix",
  description: "Run the existing privileged reflink support and refusal checks",
  localActionPath: "ci/native-filesystems",
  inputs: {},
  outputs: {},
  run: async ({ exec }) => {
    await checkFilesystems(exec, process.env.PATH);
    return {};
  },
});
