import assert from "node:assert/strict";
import { test } from "node:test";
import type { ScriptExec } from "@dedalus-labs/hollywood/action-runtime";
import {
  requestedNumber,
  requireEligibility,
  resolveRequest,
  validateDispatchedPR,
  type ResolveInput,
} from "./vouch-actions.ts";

const input: ResolveInput = {
  event: "workflow_dispatch",
  dispatchPr: "7",
  eventPr: "99",
  repository: "windsornguyen/cowtree",
  sha: "a".repeat(40),
};
const currentPR = {
  state: "open",
  head: { sha: input.sha, repo: { full_name: input.repository } },
};

test("request number comes only from the selected supported event", () => {
  assert.equal(requestedNumber("workflow_dispatch", "7", "99"), "7");
  assert.equal(requestedNumber("pull_request_target", "7", "99"), "99");
  assert.throws(() => requestedNumber("pull_request", "7", "99"), /Unsupported Vouch event/);
  for (const number of ["", "0", "-1", "01", "1.0", "1e2", " 7", "7\n", "7; true"]) {
    assert.throws(() => requestedNumber("workflow_dispatch", number, "99"), /positive integer/);
    assert.throws(() => requestedNumber("pull_request_target", "7", number), /positive integer/);
  }
});

test("manual runs resolve exactly the selected open same-repository head", async () => {
  let calls = 0;
  const exec: ScriptExec = async (file, args, options) => {
    calls += 1;
    assert.equal(file, "gh");
    assert.deepEqual(args, ["api", "repos/windsornguyen/cowtree/pulls/7"]);
    assert.deepEqual(options, { output: "capture" });
    return { exitCode: 0, stdout: JSON.stringify(currentPR), stderr: "" };
  };
  assert.equal(await resolveRequest(exec, input), "7");
  assert.equal(calls, 1);
  assert.equal(await resolveRequest(exec, { ...input, event: "pull_request_target" }), "99");
  assert.equal(calls, 1);
});

test("closed, stale, foreign, deleted, and malformed PR heads cannot authorize dispatch", () => {
  const invalid: unknown[] = [
    null,
    [],
    {},
    { ...currentPR, state: "closed" },
    { ...currentPR, head: null },
    { ...currentPR, head: { ...currentPR.head, sha: "b".repeat(40) } },
    { ...currentPR, head: { ...currentPR.head, repo: { full_name: "fork/cowtree" } } },
    { ...currentPR, head: { ...currentPR.head, repo: null } },
  ];
  for (const response of invalid) {
    assert.throws(
      () => validateDispatchedPR(response, input.repository, input.sha),
      /current same-repository head branch/,
    );
  }
  validateDispatchedPR(currentPR, input.repository, input.sha);
});

test("invalid workflow identities never invoke gh and failed queries never grant access", async () => {
  let calls = 0;
  const failed: ScriptExec = async () => {
    calls += 1;
    throw new Error("GitHub unavailable");
  };
  for (const invalid of [
    { ...input, dispatchPr: "7\n" },
    { ...input, repository: "owner/repo/extra" },
    { ...input, sha: "main" },
  ]) {
    await assert.rejects(resolveRequest(failed, invalid));
  }
  assert.equal(calls, 0);
  await assert.rejects(resolveRequest(failed, input), /GitHub unavailable/);
  const malformed: ScriptExec = async () => ({ exitCode: 0, stdout: "{", stderr: "" });
  await assert.rejects(resolveRequest(malformed, input), SyntaxError);
});

test("eligibility accepts only the two upstream success outcomes", () => {
  for (const status of ["vouched", "skipped"]) requireEligibility(status);
  for (const status of ["", "approved", "denied", "VOUCHED", "skipped\n"]) {
    assert.throws(() => requireEligibility(status), /Vouch denied contributor access/);
  }
});
