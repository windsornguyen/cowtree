import { expr, github, job, stepOutput, uses, workflow } from "@dedalus-labs/hollywood";
import { checkout } from "./steps.ts";
import { requireVouch, resolveVouchPr } from "./vouch-actions.ts";

const trustedBranch = expr<string>("github.event.repository.default_branch");
const vouchRevision = "d66fa29a64600490892131ad87597c30c91fcac4"; // v1.5.0

export const checkVouch = workflow(
  {
    name: "Vouch: Check PR",
    on: {
      pull_request_target: { types: ["opened", "reopened", "synchronize"] },
      workflow_dispatch: {
        inputs: {
          "pr-number": {
            description: "Open PR whose same-repository head branch is selected for this run",
            required: true,
            type: "number",
          },
        },
      },
    },
    permissions: { "pull-requests": "write", contents: "read" },
    jobs: {
      check: job({
        name: "Vouch eligibility",
        "runs-on": "ubuntu-24.04",
        steps: [
          // Both the local actions and contribution policy come from the trusted default branch.
          {
            ...checkout,
            name: "Read the trusted contribution policy and actions",
            with: { ref: trustedBranch, "persist-credentials": false },
          },
          uses(resolveVouchPr, {
            id: "request",
            env: { GH_TOKEN: github.token },
            with: {
              event: github.eventName,
              dispatchPr: expr<string>("inputs.pr-number"),
              eventPr: expr<string>("github.event.pull_request.number"),
              repository: github.repository,
              sha: github.sha,
            },
          }),
          {
            uses: `mitchellh/vouch/action/check-pr@${vouchRevision}`,
            id: "vouch",
            env: { GITHUB_TOKEN: github.token },
            with: {
              "pr-number": stepOutput<string>("request", "number"),
              "require-vouch": true,
              "auto-close": true,
              "vouched-file": "VOUCHED.td",
              "template-file": ".github/VOUCH_RESPONSE.md",
            },
          },
          uses(requireVouch, { with: { status: stepOutput<string>("vouch", "status") } }),
        ],
      }),
    },
  },
  { filename: "vouch-check.yml" },
);

export const manageVouch = workflow(
  {
    name: "Vouch: Manage",
    on: { issue_comment: { types: ["created"] } },
    concurrency: { group: "vouch-manage", "cancel-in-progress": false },
    permissions: { contents: "write", issues: "write", "pull-requests": "write" },
    jobs: {
      manage: job({
        if: expr<boolean>("github.event.issue.pull_request == null"),
        "runs-on": "ubuntu-24.04",
        steps: [
          { ...checkout, with: { ref: trustedBranch } },
          {
            uses: `mitchellh/vouch/action/manage-by-issue@${vouchRevision}`,
            env: { GITHUB_TOKEN: github.token },
            with: {
              repo: github.repository,
              "issue-id": expr<string>("github.event.issue.number"),
              "comment-id": expr<string>("github.event.comment.id"),
              "vouched-file": "VOUCHED.td",
              roles: "admin,maintain,write",
            },
          },
        ],
      }),
    },
  },
  { filename: "vouch-manage.yml" },
);
