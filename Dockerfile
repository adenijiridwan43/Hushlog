# A minimal, reproducible image for hushlog.
#
# The binary is built against musl in the builder stage, so it is fully static
# and can run on a `FROM scratch` runtime with no libc, no shell and no package
# manager. That keeps the image under ~10 MB and means the only thing in it is
# the tool. CI builds and pushes this to ghcr.io on every release; see
# docs/DISTRIBUTION.md.
#
# Build locally:
#   docker build -t hushlog .
#   echo "error: example" | docker run --rm -i hushlog
#
# The image is intended for CI containers: mount or pipe a log into it and it
# writes the filtered result to stdout, exactly like the binary does.

FROM rust:1.85-alpine AS builder
# Static linking needs musl-dev; the workspace itself needs nothing else.
RUN apk add --no-cache musl-dev
WORKDIR /src
# Copy the whole workspace: the crates share a lockfile and a workspace manifest,
# so a partial copy would not resolve. The build cache is warmed by the system
# layer above, which is the expensive part.
COPY . .
RUN cargo build --release --locked -p hushlog \
    && strip target/release/hushlog \
    && cp target/release/hushlog /hushlog

FROM scratch
# The static musl binary is the whole image.
COPY --from=builder /hushlog /hushlog
# hushlog reads stdin and writes stdout when given no command, which is the
# pipeline shape: `echo "$LOG" | docker run -i hushlog`.
ENTRYPOINT ["/hushlog"]
