# Ultra-Review

Several models from different providers review the same change on their own,
then cross-examine each other's findings, with evidence, until only what holds
up is left. A Chair writes the verdict. It lives in a session's **Ultra-Review**
tab, next to Big Thing.

## How a review runs

1. **Checkout.** The change is checked out in a detached worktree under
   ThingMaker's data folder: a pull request from `origin`'s `pull/N/head` (the
   open PRs are listed through the GitHub CLI, `gh`), or the current branch
   with its uncommitted changes. Reviewers read it and run its tests there, so
   the user's own tree is never touched. The checkout is removed when the
   review ends.
2. **Blind review.** Every reviewer reviews the change alone, through its lens
   (architecture, correctness, tests, security, performance, readability,
   devil's advocate, general), and reports at most eight findings.
3. **Cross-examination.** Each finding goes to every other reviewer, who must
   confirm, refine or challenge it, with evidence. Later rounds only ask about
   threads that moved.
4. **Rebuttal.** Authors answer the challenges to their findings: defend,
   accept a refinement, or withdraw. Cross-examination and rebuttal repeat for
   up to three rounds, and stop early once every finding is settled.
5. **Verdict.** The Chair decides each finding: confirmed (with its fix),
   refuted, split (a real disagreement, with the best case for and against,
   for the user to settle) or withdrawn; then recommends approve, approve
   after fixes, or request changes.

**Quick review** runs one round of cross-examination, no rebuttal, and
reviewers read the code without running tests.

## Rules the engine enforces

- Reviewers know each other only by letter (Reviewer A, B…), in every round.
  The user can see which model holds each letter; the reviewers cannot.
- Nobody is asked about, or can answer, their own finding.
- A reviewer is pinned to its seat: when its account hits a limit, its turn
  waits the limit out instead of moving to another reviewer.
- Every turn ends in a fenced JSON block, which every provider can write. A
  reply that cannot be read is asked for once more; a reviewer whose turns keep
  failing drops out and the review goes on without it. If the Chair cannot
  write the verdict, it is summed up from where each finding stood.
- The user can step into any finding's thread; reviewers read the note in their
  next turn, and the Chair weighs it.

## After the verdict

Post it on the pull request as a review comment (from the user's own `gh`
account, after confirming), copy it as Markdown, or put the confirmed findings
in the session's prompt for its agent to fix. Each review records how every
seat's findings held up, summed across reviews by provider, model and lens.

## Code

- `crates/thingmaker-supervisor/src/ultrareview/`: the model, the debate's
  rules, prompts and parsing, the checkout and `gh`, the engine, and the
  Markdown write-up. Reviewers are delegation workers under the key
  `ultra-review:<id>`.
- `apps/desktop/src-tauri/src/commands/ultrareview.rs`: the commands, and the
  `thingmaker://ultra-review` event that carries every change to a review.
- `apps/desktop/src/components/UltraReview.tsx` and `UltraReviewDebate.tsx`:
  the set-up, agora and verdict screens; `apps/desktop/src/ultraReview.ts`: the
  renderer's helpers.
