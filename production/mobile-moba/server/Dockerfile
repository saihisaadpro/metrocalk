# Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
#
# MOB-2 match-server CANDIDATE image. This packages the deterministic host binary and nothing else. It has
# no listener, no daemon mode, no persistence, no control plane, and no orchestration integration; those
# remain explicit later gates. Image signing, SBOM generation, vulnerability scanning, registry publication,
# and orchestrated-drain evidence are NOT performed here and are still acceptance blockers.
#
# The build context is the repository root, but only the two dependency-free crates are copied in. Both
# `metrocalk-gameplay` and `metrocalk-match-server` have zero foreign dependencies, so the container build
# resolves entirely from path dependencies and runs with `--offline`: no crate registry is contacted, and
# the image cannot be silently influenced by a registry at build time.

# Pinned to the workspace MSRV (`rust-version = "1.92"`), which is also the toolchain that produced the
# locally verified evidence-parity binary, so the image is built by the compiler this candidate was checked
# against rather than by whatever "stable" happens to be on the day CI runs. A digest pin is the next
# hardening step and is still listed as an acceptance blocker, not a claim made here.
FROM rust:1.92-slim-bookworm AS builder

WORKDIR /build

# A purpose-built two-member workspace. The repository root workspace pulls in the editor, renderer, and
# Flecs C core, none of which belong in a headless match-server image.
COPY production/mobile-moba/server/container-workspace.toml ./Cargo.toml
COPY gameplay ./gameplay
COPY match-server ./match-server

# `--locked` is deliberately absent: there is no checked-in lockfile for this synthetic workspace, and with
# zero external dependencies the resolved graph is exactly the two local crates. `--offline` is what makes
# that guarantee enforceable rather than assumed — if either crate ever grows a registry dependency, this
# build fails loudly instead of quietly reaching out to crates.io.
RUN cargo build --release --offline -p metrocalk-match-server

# distroless/cc carries glibc and libgcc_s for the Rust binary and nothing else: no shell, no package
# manager, no interpreters. `nonroot` runs as uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=builder /build/target/release/metrocalk-match-server /usr/local/bin/metrocalk-match-server

USER nonroot:nonroot
WORKDIR /

# The binary exposes only `--version-json`, `--smoke-json`, and `--help`, and exits after each. There is no
# default long-running command because this candidate has nothing to serve.
ENTRYPOINT ["/usr/local/bin/metrocalk-match-server"]
CMD ["--version-json"]

LABEL org.opencontainers.image.title="metrocalk-match-server" \
      org.opencontainers.image.description="MOB-2 deterministic match-server candidate; no transport, not production" \
      org.opencontainers.image.licenses="NONE" \
      com.metrocalk.artifact-status="mob2-candidate-not-production" \
      com.metrocalk.transport="none"
