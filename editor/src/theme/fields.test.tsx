//! The settings-sheet family's state matrix (ADR-147) — see `fields.tsx` for why these eight live in
//! their own module. Structured signals only: the class hook, the wired handler, the id that ties a
//! label to its control, the reason a refusing control gives. Never a resolved colour or a padding —
//! jsdom loads no stylesheet, so a test asserting one would be asserting nothing.

import { expect, test, vi } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import { Callout, Checkbox, Field, FieldGrid, ListRow, Metric, ProgressBar, Radio, RadioGroup } from "./fields";

test("Field: the visible label names the control it edits, and the help is prose under it", () => {
  render(
    <Field label="Chart padding" htmlFor="pad" help="Pixels reserved between packed islands." unit="px">
      <input id="pad" defaultValue="8" />
    </Field>,
  );
  // `getByLabelText` only resolves if `htmlFor` actually reached the control — the accessibility
  // contract, not a rendering detail.
  expect((screen.getByLabelText("Chart padding") as HTMLInputElement).value).toBe("8");
  expect(screen.getByText("Pixels reserved between packed islands.").className).toContain("mtk-field__help");
  expect(screen.getByText("px").className).toContain("mtk-field__unit");
});

test("Field: span maps to the grid modifier the stylesheet reads, and disabled marks the whole field", () => {
  render(
    <Field label="A" htmlFor="a" span="full" disabled data-testid="f">
      <input id="a" disabled />
    </Field>,
  );
  const field = screen.getByTestId("f");
  expect(field.className).toContain("mtk-field--full");
  expect(field.getAttribute("data-disabled")).toBe("true");
});

test("FieldGrid: the column floor is a custom property, so the column COUNT stays a consequence of width", () => {
  render(<FieldGrid minColumn={240} data-testid="grid"><span>x</span></FieldGrid>);
  const grid = screen.getByTestId("grid");
  expect(grid.className).toContain("mtk-field-grid");
  expect(grid.style.getPropertyValue("--mtk-field-min")).toBe("240px");
});

test("Checkbox: the row is one labelled control, and toggling reports the NEW checked state", () => {
  const onChange = vi.fn();
  render(<Checkbox label="Ambient occlusion" checked={false} onChange={onChange} />);
  const box = screen.getByRole("checkbox", { name: /Ambient occlusion/ }) as HTMLInputElement;
  expect(box.className).toContain("mtk-check__box");
  fireEvent.click(box);
  expect(onChange).toHaveBeenCalledWith(true);
});

test("Checkbox: a description is wired as the control's own description, not loose text beside it", () => {
  render(
    <Checkbox
      label="Replace UV0"
      description="Only when you will rebake every bound texture."
      checked
      onChange={vi.fn()}
    />,
  );
  const box = screen.getByRole("checkbox", { name: /Replace UV0/ });
  const describedBy = box.getAttribute("aria-describedby");
  expect(describedBy).toBeTruthy();
  expect(document.getElementById(describedBy!)?.textContent).toMatch(/rebake every bound texture/);
});

test("Checkbox: a refusing box carries its reason ON THE INPUT — a title on the label is one the control never gives", () => {
  render(
    <Checkbox label="Normal" checked disabled disabledReason="Run Inspect first." onChange={vi.fn()} />,
  );
  // The shots harness's R9 check walks UP from the refusing element through inactive ancestors only,
  // and a `<label>` is not itself disabled. This assertion is that walk, in a unit test.
  const box = screen.getByRole("checkbox", { name: /Normal/ });
  expect(box.getAttribute("title")).toBe("Run Inspect first.");
});

test("Checkbox: an enabled box makes no claim about being refused", () => {
  render(<Checkbox label="Normal" checked disabledReason="Run Inspect first." onChange={vi.fn()} />);
  expect(screen.getByRole("checkbox", { name: /Normal/ }).getAttribute("title")).toBeNull();
});

test("Callout: every tone carries a MARK as well as a class, so the meaning is never colour alone", () => {
  for (const tone of ["neutral", "info", "success", "warn", "danger"] as const) {
    const { container } = render(<Callout tone={tone} data-testid={`c-${tone}`}>note</Callout>);
    const el = screen.getByTestId(`c-${tone}`);
    expect(el.className).toContain(`mtk-callout--${tone}`);
    expect(container.querySelector(".mtk-callout__icon svg")).toBeTruthy();
    cleanup();
  }
});

test("Callout: a title reads before the body and a role can be forwarded for a live announcement", () => {
  render(<Callout tone="warn" role="alert" title="Source stays unchanged" data-testid="c">Review it first.</Callout>);
  const el = screen.getByTestId("c");
  expect(el.getAttribute("role")).toBe("alert");
  expect(el.textContent).toBe("Source stays unchangedReview it first.");
});

test("Metric: a value with no result reads once; a measured result reads as before AND after", () => {
  const { rerender } = render(<Metric label="Triangles" value="12,000" data-testid="m" />);
  expect(screen.getByTestId("m").textContent).toBe("Triangles12,000");

  rerender(<Metric label="Triangles" value="12,000" after="4,800" data-testid="m" />);
  const el = screen.getByTestId("m");
  // The word "after" is present for a screen reader, so the two numbers are not an unexplained pair.
  expect(el.textContent).toContain("12,000");
  expect(el.textContent).toContain("4,800");
  expect(el.textContent).toContain("after");
  expect(el.querySelector(".mtk-metric__value--after")?.textContent).toContain("4,800");
});

test("Metric: a unit is appended to BOTH sides of a comparison, never only the first", () => {
  render(<Metric label="Coverage" value="62.4" after="91.0" unit="%" data-testid="m" />);
  const values = [...screen.getByTestId("m").querySelectorAll(".mtk-metric__value")].map((v) => v.textContent);
  expect(values).toEqual(["62.4 %", "91.0 %"]);
});

test("ProgressBar: determinate and indeterminate are different DOM, and both are named", () => {
  const { rerender } = render(<ProgressBar value={0.4} label="Operation progress" />);
  const bar = screen.getByLabelText("Operation progress") as HTMLProgressElement;
  expect(bar.className).toContain("mtk-progress");
  expect(bar.value).toBe(0.4);

  rerender(<ProgressBar label="Operation progress" />);
  // An indeterminate `<progress>` has NO value attribute — the browser draws the sweep from its
  // absence, so setting 0 instead would render a bar that says "nothing has happened yet".
  expect(screen.getByLabelText("Operation progress").hasAttribute("value")).toBe(false);
});

test("RadioGroup: one accessible name for the question, one answer per option", () => {
  render(
    <RadioGroup label="Start state" help="Where the machine begins.">
      <Radio name="start" label="Start" ariaLabel="Set Closed as the start state" checked onChange={() => {}} />
      <Radio name="start" label="Start" ariaLabel="Set Opening as the start state" checked={false} onChange={() => {}} />
    </RadioGroup>,
  );
  const group = screen.getByRole("radiogroup", { name: "Start state" });
  expect(group.className).toContain("mtk-radio-group");
  // The QUESTION is announced once, by the group. Each option carries only its own answer, rather than
  // every row repeating the sentence.
  expect(group.getAttribute("aria-describedby")).toBe(screen.getByText("Where the machine begins.").id);
  const chosen = screen.getByRole("radio", { name: "Set Closed as the start state" }) as HTMLInputElement;
  expect(chosen.checked).toBe(true);
  expect((screen.getByRole("radio", { name: "Set Opening as the start state" }) as HTMLInputElement).checked).toBe(false);
  cleanup();
});

test("Radio: it is the shared mark, not the OS one, and it picks on change", () => {
  const onChange = vi.fn();
  render(<Radio name="pick" label="Start" checked={false} onChange={onChange} />);
  const input = screen.getByRole("radio", { name: "Start" }) as HTMLInputElement;
  // `appearance: none` is what takes it off the desktop widget, and the class is what carries it —
  // asserting the hook is asserting that the shared drawing is in play (jsdom loads no stylesheet).
  expect(input.className).toBe("mtk-radio__dot");
  expect(input.type).toBe("radio");
  fireEvent.click(input);
  expect(onChange).toHaveBeenCalledTimes(1);
  cleanup();
});

test("Radio: a refusing option states its reason ON the input, not only on the label around it", () => {
  render(
    <Radio
      name="pick"
      label="Start"
      checked={false}
      disabled
      disabledReason="A machine needs at least one state."
      onChange={() => {}}
    />,
  );
  // An assistive technology reading the radio reaches the INPUT; a `<label>` is not itself disabled, so
  // a reason parked only there is a reason the control never gives (the `Checkbox` rule, restated).
  const input = screen.getByRole("radio", { name: "Start" });
  expect(input.getAttribute("title")).toBe("A machine needs at least one state.");
  expect((input as HTMLInputElement).disabled).toBe(true);
  cleanup();
});

test("ListRow: a card row is a surface of its own, a plain row is not", () => {
  render(
    <>
      <ListRow data-testid="plain">plain</ListRow>
      <ListRow tone="card" data-testid="card">card</ListRow>
    </>,
  );
  expect(screen.getByTestId("plain").className).toBe("mtk-list-row");
  expect(screen.getByTestId("card").className).toContain("mtk-list-row--card");
  cleanup();
});
