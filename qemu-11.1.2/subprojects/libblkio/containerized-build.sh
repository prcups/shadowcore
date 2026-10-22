#!/bin/sh
cd "$(dirname $0)"

# Build image, if it doesn't exist or is older than ./Containerfile
if [ $(podman images --noheading --filter reference=libblkio-builder | wc -l) -eq 0 ]; then
    buildah bud --tag libblkio-builder .
elif [ $(podman images --noheading --filter reference=libblkio-builder --filter until=$(date -r ./Containerfile +%s) | wc -l) -ne 0 ]; then
    buildah bud --tag libblkio-builder .
fi

# Disable seccomp so io_uring syscalls are allowed
exec podman run --security-opt=seccomp=unconfined --rm -it --user "$(id --user):$(id --group)" --userns keep-id --volume .:/usr/src/libblkio:z libblkio-builder ./build.sh
