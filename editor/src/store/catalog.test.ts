//! The catalog vocabulary projection (ADR-144) — pinned against the SAME strings `core/src/caps.rs` and
//! `core/src/taxonomy.rs` pin on the Rust side, because this is one rule stated twice in two languages
//! and only a shared example can compare them (`<test_and_ci_discipline>` 6).

import { expect, test } from "vitest";
import { CATALOG_ICONS, bucketIcon, bucketLabel, catalogTier, displayName } from "./catalog";
import { resolveIcon } from "../theme/icons";

test("a std: namespace is dropped and any other is parenthesised — the mirror of caps::display_name", () => {
  // `core/src/caps.rs` tests: `canonical("Health") == "std:Health"`, and `display_name` renders the
  // standard ones bare and the namespaced ones with their author in parentheses.
  expect(displayName("std:Health")).toBe("Health");
  expect(displayName("std:Renderable")).toBe("Renderable");
  expect(displayName("acme:Shield")).toBe("Shield (acme)");
  expect(displayName("brandx:Familiars")).toBe("Familiars (brandx)");
  // Already display form — an un-namespaced name is returned untouched rather than mangled.
  expect(displayName("Spatial")).toBe("Spatial");
  // Degenerate input keeps its own text: a name that is only a namespace is not silently emptied.
  expect(displayName("std:")).toBe("std:");
  expect(displayName("")).toBe("");
});

test("a bucket heading is the display name of the bucket, which is what the panel was NOT showing", () => {
  // `core/src/taxonomy.rs` tests: `Category::std("Props").bucket() == "std:Props"`, and two authors'
  // aliased categories both bucket under `std:Characters`. Those keys are what `catalog()` sends.
  expect(bucketLabel("std:Props")).toBe("Props");
  expect(bucketLabel("std:Characters")).toBe("Characters");
  expect(bucketLabel("std:Other")).toBe("Other");
});

test("the two tiers the core serialises are named, and an unknown one keeps its own word", () => {
  // `metrocalk_core::catalog::Source` serialises exactly these two.
  expect(catalogTier("local").metered).toBe(false);
  expect(catalogTier("marketplace").metered).toBe(true);
  expect(catalogTier("local").label).not.toBe("local"); // projected into a word, not echoed
  // The word is short because it shares a ~110px line; the sentence it stands for is the tooltip.
  expect(catalogTier("marketplace").label.length).toBeLessThanOrEqual(12);
  expect(catalogTier("marketplace").hint).toContain("tokens");
  // The resolver's escalation tiers arrive on the same field (ADR-012) and are named too.
  expect(catalogTier("generated").metered).toBe(true);
  expect(catalogTier("imported").metered).toBe(false);
  // A tier this build has never heard of must not be renamed into a neighbouring one — that is how a
  // future source gets reported to a user as something it is not. It keeps its word and cannot spend.
  const unknown = catalogTier("consortium");
  expect(unknown.icon).toBe("default");
  expect(unknown.label).toBe("consortium");
  expect(unknown.metered).toBe(false);
});

test("every mark this module can ask for actually resolves to a drawing", () => {
  // `Icon`'s `name` is a `string` because the Rust catalogs feed it at runtime, so `tsc` cannot
  // spell-check these; and `check-icon-vocab.mjs` reads LITERAL `<Icon name>` sites, which a table
  // lookup is not. A typo here is a blank square in a grid of previews — the ADR-131 defect exactly.
  expect(CATALOG_ICONS.length).toBeGreaterThan(0);
  for (const name of CATALOG_ICONS) expect([name, resolveIcon(name)]).not.toEqual([name, null]);
  // The seven buckets `metrocalk_core::taxonomy` defines, each with a mark of its own.
  for (const bucket of ["std:UI", "std:Gameplay", "std:Props", "std:Characters", "std:Audio", "std:Logic", "std:Other"]) {
    expect(resolveIcon(bucketIcon(bucket))).not.toBeNull();
  }
  // A bucket a future core adds falls back to a real drawing rather than to an empty box.
  expect(resolveIcon(bucketIcon("std:Vehicles"))).not.toBeNull();
});
