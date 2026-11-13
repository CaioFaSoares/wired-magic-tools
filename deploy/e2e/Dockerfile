FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends iproute2 python3 curl ca-certificates procps iptables \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/wm"]
CMD ["--help"]
