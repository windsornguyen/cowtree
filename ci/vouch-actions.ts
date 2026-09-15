import {
  action,
  choiceInput,
  stringInput,
  stringOutput,
  type ActionInputValues,
  type ScriptExec,
} from "@dedalus-labs/hollywood/action-runtime";

const resolveInputs = {
  event: choiceInput({
    description: "Vouch workflow event",
    options: ["workflow_dispatch", "pull_request_target"] as const,
  }),
  dispatchPr: stringInput({ description: "Manually requested PR", default: "" }),
  eventPr: stringInput({ description: "Pull request event number", default: "" }),
  repository: stringInput({ description: "Repository receiving the workflow" }),
  sha: stringInput({ description: "Commit selected for the workflow run" }),
} as const;

export type ResolveInput = ActionInputValues<typeof resolveInputs>;

export function requestedNumber(event: string, dispatchPr: string, eventPr: string): string {
  let number: string;
  switch (event) {
    case "workflow_dispatch":
      number = dispatchPr;
      break;
    case "pull_request_target":
      number = eventPr;
      break;
    default:
      throw new Error(`Unsupported Vouch event: ${event}`);
  }
  if (!/^[1-9][0-9]*$/.test(number)) {
    throw new Error("pr-number must be a positive integer");
  }
  return number;
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function validateDispatchedPR(value: unknown, repository: string, sha: string): void {
  if (
    !record(value) ||
    value.state !== "open" ||
    !record(value.head) ||
    value.head.sha !== sha ||
    !record(value.head.repo) ||
    value.head.repo.full_name !== repository
  ) {
    throw new Error("Select this open PR's current same-repository head branch");
  }
}

export async function resolveRequest(exec: ScriptExec, input: ResolveInput): Promise<string> {
  const number = requestedNumber(input.event, input.dispatchPr, input.eventPr);
  if (input.event === "pull_request_target") return number;
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(input.repository)) {
    throw new Error("Expected the workflow repository in owner/name form");
  }
  if (!/^[0-9a-f]{40}$/.test(input.sha)) {
    throw new Error("Expected the workflow's full commit SHA");
  }
  const result = await exec("gh", ["api", `repos/${input.repository}/pulls/${number}`], {
    output: "capture",
  });
  const response: unknown = JSON.parse(result.stdout);
  validateDispatchedPR(response, input.repository, input.sha);
  return number;
}

export const resolveVouchPr = action({
  name: "Resolve the requested PR",
  description: "Validate the PR identity before checking contributor access",
  localActionPath: "ci/resolve-vouch-pr",
  inputs: resolveInputs,
  outputs: { number: stringOutput({ description: "Validated pull request number" }) },
  run: async ({ exec, input }) => ({ number: await resolveRequest(exec, input) }),
});

export function requireEligibility(status: string): void {
  if (status !== "vouched" && status !== "skipped") {
    throw new Error(`Vouch denied contributor access: ${status}`);
  }
}

export const requireVouch = action({
  name: "Require contributor access",
  description: "Reject every outcome except vouched or collaborator access",
  localActionPath: "ci/require-vouch",
  inputs: { status: stringInput({ description: "Vouch action status" }) },
  outputs: {},
  run: async ({ input, log }) => {
    requireEligibility(input.status);
    log.info(`Vouch result: ${input.status}`);
    return {};
  },
});
