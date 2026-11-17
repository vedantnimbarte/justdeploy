# The builder: clones and builds apps (with the host's Docker via its socket),
# and runs health checks on project networks.
FROM alpine:3.22

ARG RAILPACK_VERSION=0.43.0
ARG TARGETARCH

RUN apk add --no-cache bash ca-certificates coreutils curl docker-cli docker-cli-buildx git

RUN case "$TARGETARCH" in \
      amd64) arch=x86_64 ;; \
      arm64) arch=arm64 ;; \
      *) echo "unsupported architecture: $TARGETARCH" && exit 1 ;; \
    esac \
 && curl -fsSL "https://github.com/railwayapp/railpack/releases/download/v${RAILPACK_VERSION}/railpack-v${RAILPACK_VERSION}-${arch}-unknown-linux-musl.tar.gz" \
    | tar -xz -C /usr/local/bin railpack \
 && railpack --version

ENV RAILPACK_VERSION=${RAILPACK_VERSION}
COPY build.sh healthcheck.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/build.sh /usr/local/bin/healthcheck.sh
