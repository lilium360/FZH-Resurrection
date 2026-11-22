# shadNet (the PSN replacement for shadPS4), built from source for x86-64 or ARM64 Linux.
# Upstream: https://github.com/shadps4-emu/shadNet (GPL-2.0-or-later), used unmodified.
# Debian 13 is the base because shadNet needs Qt >= 6.8 (QHttpServer::bind); Ubuntu 24.04 ships 6.4.

FROM debian:trixie-slim AS build

ARG SHADNET_COMMIT=b38cf175ed2215456972356196f46b82c171e987

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates git cmake ninja-build g++ \
        qt6-base-dev qt6-httpserver-dev qt6-websockets-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
RUN git clone https://github.com/shadps4-emu/shadNet.git . \
    && git checkout "$SHADNET_COMMIT" \
    && git submodule update --init --recursive --depth 1

RUN cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
    && cmake --build build --parallel "$(nproc)" --target shadnet \
    && strip build/shadnet

FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        libqt6core6t64 libqt6network6 libqt6sql6 libqt6sql6-sqlite \
        libqt6concurrent6 libqt6httpserver6 libqt6websockets6 \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Qt warns at every start without a UTF-8 locale.
ENV LANG=C.UTF-8

COPY --from=build /src/build/shadnet /opt/shadnet/shadnet
COPY --from=build /src/worlds.cfg /src/scoreboards.cfg /src/clientversions.cfg /opt/shadnet/defaults/
COPY entrypoint.sh /opt/shadnet/entrypoint.sh
RUN sed -i 's/\r$//' /opt/shadnet/entrypoint.sh && chmod +x /opt/shadnet/entrypoint.sh

# shadnet.cfg, db/, logs and score data.
VOLUME /data
# 31313 binary protocol (TCP), 31314 matching/STUN (UDP), 31315 WebAPI (TCP).
EXPOSE 31313/tcp 31314/udp 31315/tcp

ENTRYPOINT ["/opt/shadnet/entrypoint.sh"]
