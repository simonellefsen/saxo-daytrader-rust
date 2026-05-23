#!/bin/bash

docker pull rustfs/rustfs:latest || :
docker stop daytrader_rustfs || :
docker rm daytrader_rustfs || :
docker run -d \
  --publish 9000:9000 \
  --publish 9001:9001 \
  --volume /Users/lindau/codex/rust_daytrader/rustfs:/data \
  --env RUSTFS_ADDRESS=:9000 \
  --env RUSTFS_SERVER_DOMAINS=host.docker.internal \
  --env RUSTFS_ALLOW_INSECURE_DEFAULT_CREDENTIALS=true \
  --env RUSTFS_ACCESS_KEY=rustfsadmin \
  --env RUSTFS_SECRET_KEY=rustfsadmin \
  --env RUSTFS_CONSOLE_ENABLE=true \
  --name daytrader_rustfs \
  rustfs/rustfs:latest \
  /data
