# PostgreSQL with wal-g, for point-in-time recovery. Databases switch to it only
# when point-in-time recovery is turned on; it is the stock alpine image with one
# extra binary, so their data directory doesn't change.
FROM golang:1.27-alpine AS walg
ARG WALG_VERSION=v3.0.10
RUN apk add --no-cache git
RUN git clone --depth 1 --branch "${WALG_VERSION}" https://github.com/wal-g/wal-g /src
WORKDIR /src
# No cgo: one static binary that runs on alpine (musl) as well as glibc images.
RUN CGO_ENABLED=0 go build -trimpath -ldflags "-s -w" -o /wal-g ./main/pg \
 && /wal-g --version

FROM postgres:17-alpine
COPY --from=walg /wal-g /usr/local/bin/wal-g
