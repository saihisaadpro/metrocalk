import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import { fakeClient } from "../transport/test-client";
import { OnlyIfBlock } from "./OnlyIfBlock";

afterEach(() => {
  act(() => {
    projectionStore.getState().reset();
    playStore.getState().reset();
  });
  vi.restoreAllMocks();
});

function select(id: string, name: string) {
  act(() => {
    projectionStore.getState().applyDelta({ ops: [{ op: "upsert", id, name, parentId: null }] });
    projectionStore.getState().select(id);
  });
}

const ROSTER = [
  { entity: "coin", name: "Coin", role: "collectible" },
  { entity: "key", name: "Key", role: "collectible" },
];

test("the role's own clause is shown with provenance, and the rule reads as a sentence", async () => {
  const client = fakeClient({
    conditionList: vi.fn(() =>
      Promise.resolve({
        all: [{ reads: "the Score is at least 3", index: 0, any: false }],
        any: [],
        roleClause: "it hasn't been collected yet",
        sentence:
          "When something touches this object, only if it hasn't been collected yet and the Score is at least 3, collect it and add to the Score.",
      }),
    ),
  });
  select("coin", "Coin");
  render(<OnlyIfBlock client={client} roster={ROSTER} />);

  // WAIT FOR THE CONTENT, NOT FOR THE CONTAINER. `onlyif-block` renders on the first frame with its
  // empty prompt inside it, so `findByTestId` resolves before `conditionList()` has resolved and a
  // `textContent` assertion taken straight after it is racing that promise. It won on an idle machine
  // and lost under a loaded one, which is a flake with a cause rather than noise.
  const block = await screen.findByTestId("onlyif-block");
  // Reading the clause your ROLE already wrote is the first exposure to conditionals.
  await waitFor(() => expect(block.textContent).toContain("it hasn't been collected yet (from the role)"));
  expect(block.textContent).toContain("the Score is at least 3");
  const sentence = await screen.findByTestId("onlyif-sentence");
  expect(sentence.textContent).toContain("only if");
  expect(sentence.textContent).toContain("collect it");
});

test("picking a card that needs a number reveals the value field and adds one clause", async () => {
  const client = fakeClient();
  select("coin", "Coin");
  render(<OnlyIfBlock client={client} roster={ROSTER} />);

  const pick = (await screen.findByTestId("onlyif-pick")) as HTMLSelectElement;
  // Nothing is asked for until a card is chosen — zero-config by default.
  expect(screen.queryByTestId("onlyif-number")).toBeNull();
  fireEvent.change(pick, { target: { value: "score_at_least" } });
  const num = (await screen.findByTestId("onlyif-number")) as HTMLInputElement;
  fireEvent.change(num, { target: { value: "3" } });
  fireEvent.blur(num);
  fireEvent.click(await screen.findByTestId("onlyif-add"));

  await waitFor(() => {
    expect(client.conditionAdd).toHaveBeenCalledWith("coin", {
      kind: "score_at_least",
      number: 3,
      object: null,
      any: false,
    });
  });
});

test("an object card only offers OTHER role-carrying objects, never itself", async () => {
  const client = fakeClient();
  select("coin", "Coin");
  render(<OnlyIfBlock client={client} roster={ROSTER} />);
  fireEvent.change(await screen.findByTestId("onlyif-pick"), { target: { value: "other_gone" } });
  const picker = (await screen.findByTestId("onlyif-object")) as HTMLSelectElement;
  const options = [...picker.options].map((o) => o.textContent);
  expect(options).toEqual(["Key"]);
});

test("the alternative toggle switches the clause into the OR group", async () => {
  const client = fakeClient();
  select("coin", "Coin");
  render(<OnlyIfBlock client={client} roster={ROSTER} />);
  fireEvent.change(await screen.findByTestId("onlyif-pick"), { target: { value: "still_active" } });
  fireEvent.click(await screen.findByTestId("onlyif-join"));
  fireEvent.click(await screen.findByTestId("onlyif-add"));
  await waitFor(() => {
    expect(client.conditionAdd).toHaveBeenCalledWith(
      "coin",
      expect.objectContaining({ any: true }),
    );
  });
});

test("a refusal is shown inline in plain language, and nothing is claimed to have happened", async () => {
  const client = fakeClient({
    conditionAdd: vi.fn(() =>
      Promise.resolve({
        applied: null,
        entity: "coin",
        added: [],
        scoreEntity: null,
        message: "",
        reason: "there is no Score yet — make something a Collectible first, and one appears",
      }),
    ),
  });
  select("coin", "Coin");
  render(<OnlyIfBlock client={client} roster={ROSTER} />);
  fireEvent.change(await screen.findByTestId("onlyif-pick"), { target: { value: "score_at_least" } });
  fireEvent.click(await screen.findByTestId("onlyif-add"));
  const refusal = await screen.findByTestId("onlyif-refusal");
  expect(refusal.textContent).toContain("no Score yet");
});

test("during Play the near-miss line answers 'nothing happened' with the live value", async () => {
  const client = fakeClient();
  select("coin", "Coin");
  act(() => playStore.getState().refresh({ playing: true, paused: false }));
  render(
    <OnlyIfBlock client={client} roster={ROSTER} blockedWhy="the Score is 0, needs at least 3" />,
  );
  const blocked = await screen.findByTestId("onlyif-blocked");
  expect(blocked.textContent).toContain("Blocked just now");
  expect(blocked.textContent).toContain("the Score is 0, needs at least 3");
  // Authoring is closed during a run — the clause editor is not offered.
  expect(screen.queryByTestId("onlyif-pick")).toBeNull();
});
