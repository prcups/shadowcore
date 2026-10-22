#!/bin/bash

set -o errexit -o pipefail -o nounset
export LANG=C

start_time="$( date +%s.%N )"
repo_root="$( dirname "$0" | xargs readlink -e )"

function __log() {
    # shellcheck disable=SC2059
    printf "\033[%sm[%6.1f] %s\033[0m\n" \
        "$1" \
        "$( bc -l <<< "$( date +%s.%N ) - ${start_time}" )" \
        "$( printf "${@:2}" )"
}

function __log_info()    { __log 36 "$@"; }
function __log_success() { __log 32 "$@"; }
function __log_failure() { __log 31 "$@"; }

function __is_any_of() {
    for item in "${@:2}"; do
        if [[ "$1" = "${item}" ]]; then
            return 0
        fi
    done
    return 1
}

# check usage

supported_suites=(
    io_uring
    nvme-io_uring
    nvme-io_uring-zoned
    virtio-blk-vfio-pci
    virtio-blk-vhost-user
    virtio-blk-vhost-vdpa
    )

function __bad_usage() {
    >&2 echo -n "\
Usage: $0 <test_suites...>
       $0 all

Run the given test suites in a VM providing the necessary environment. The VM
image is automatically created and cached for future runs.

Supported test suites:
$( printf '  - %s\n' "${supported_suites[@]}" )

If invoked with a single \`all\` argument, all supported test suites are run.
"
    exit 2
}

if (( $# == 0 )); then
    __bad_usage
elif (( $# == 1 )) && [[ "$1" = all ]]; then
    enabled_suites=( "${supported_suites[@]}" )
else
    for arg in "$@"; do
        if ! __is_any_of "${arg}" "${supported_suites[@]}"; then
            __bad_usage
        fi
    done
    enabled_suites=( "$@" )
fi

function __suite_is_enabled() {
    __is_any_of "$1" "${enabled_suites[@]}"
}

# check if required commands are available

function __ensure_command_exists() {
    if ! command -v "$1" >& /dev/null; then
        >&2 echo "Command '$1' not found."
        exit 1
    fi
}

__ensure_command_exists virt-builder
__ensure_command_exists qemu-img
__ensure_command_exists qemu-system-x86_64

# create temporary directory

temp_dir="$( mktemp -d )"
trap 'rm -fr "${temp_dir}"' EXIT

# create base immutable guest image

base_image="${repo_root}/test-image.qcow2"

if [[ ! -e "${base_image}" ]]; then

    __log_info 'Creating guest base image...'

    # NOTE: SELinux prevents access to /dev/ng0n1 through uring-cmd, so we just
    # set it to permissive mode.

    virt-builder \
        fedora-38 \
        --smp "$(( $( nproc ) / 2 ))" \
        --memsize 4096 \
        --output "${base_image}" \
        --format qcow2 \
        --arch x86_64 \
        --hostname libblkio-test-vm \
        --root-password password:root \
        --install cargo,driverctl,htop,iproute,meson,pciutils,python3-docutils,qemu-img,rust \
        --append-line '/etc/fstab:libblkio /root/libblkio 9p trans=virtio,version=9p2000.L,ro 0 0' \
        --append-line '/etc/ssh/sshd_config:PermitRootLogin yes' \
        --append-line '/etc/ssh/sshd_config:PubkeyAuthentication no' \
        --write '/etc/ssh/sshd_config.d/10-myconf.conf:GSSAPIAuthentication no' \
        --append-line '/etc/ssh/sshd_config.d/10-myconf.conf:ChallengeResponseAuthentication yes' \
        --append-line '/etc/default/grub:GRUB_CMDLINE_LINUX_DEFAULT="intel_iommu=on"' \
        --run-command 'grub2-mkconfig -o /boot/grub2/grub.cfg' \
        --run-command 'sed -i s/SELINUX=enforcing/SELINUX=permissive/ /etc/selinux/config'

fi

# create temporary overlay image

__log_info 'Creating guest overlay image...'
overlay_image="${temp_dir}/overlay-image.qcow2"
qemu-img create -f qcow2 -b "${base_image}" -F qcow2 "${overlay_image}"

# boot guest

# TODO: Check if using virtio-fs instead of 9pfs makes things faster.

# shellcheck disable=SC2054
qemu_args=(
    --display none
    --machine q35,accel=kvm,kernel-irqchip=split
    --cpu host
    --smp cores="$(( $( nproc ) / 2 ))"
    -m 4G

    # forwarding is so we can ssh into the guest
    --nic user,model=virtio-net-pci-non-transitional,hostfwd=tcp::12345-:22

    # boot device
    --blockdev driver=file,node-name=boot-file,filename="${overlay_image}"
    --blockdev driver=qcow2,node-name=boot-qcow2,file=boot-file
    --device virtio-blk-pci-non-transitional,drive=boot-qcow2,bootindex=1

    # an IOMMU
    --device intel-iommu,intremap=on,device-iotlb=on

    # share libblkio repo with guest
    --virtfs local,path="${repo_root}",mount_tag=libblkio,security_model=mapped-xattr,id=libblkio,readonly=on
)

if __suite_is_enabled io_uring ||
    __suite_is_enabled virtio-blk-vfio-pci ||
    __suite_is_enabled virtio-blk-vhost-user; then

    virtio_blk_pci_data_file="${temp_dir}/virtio-blk-pci.dat"
    truncate --size 128MiB "${virtio_blk_pci_data_file}"

    virtio_blk_packed_pci_data_file="${temp_dir}/virtio-blk-packed-pci.dat"
    truncate --size 128MiB "${virtio_blk_packed_pci_data_file}"

    # shellcheck disable=SC2054
    qemu_args+=(
        # virtio-blk-pci test device
        --blockdev driver=file,node-name=virtio-blk-pci-file,filename="${virtio_blk_pci_data_file}"
        --device virtio-blk-pci-non-transitional,drive=virtio-blk-pci-file,iommu_platform=on,addr=05.0,packed=off
        # virtio-blk-pci (packed vq) test device
        --blockdev driver=file,node-name=virtio-blk-packed-pci-file,filename="${virtio_blk_packed_pci_data_file}"
        --device virtio-blk-pci-non-transitional,drive=virtio-blk-packed-pci-file,iommu_platform=on,addr=08.0,packed=on
    )

fi

if __suite_is_enabled nvme-io_uring; then

    nvme_data_file="${temp_dir}/nvme.dat"
    truncate --size 128MiB "${nvme_data_file}"

    # shellcheck disable=SC2054
    qemu_args+=(
        # NVMe PCI test device
        --blockdev driver=file,node-name=nvme-file,filename="${nvme_data_file}"
        --device nvme,drive=nvme-file,serial=deadbeef,addr=06.0
    )

fi

if __suite_is_enabled nvme-io_uring-zoned; then

    nvme_data_file="${temp_dir}/nvme-zoned.dat"
    truncate --size 128MiB "${nvme_data_file}"

    # shellcheck disable=SC2054
    qemu_args+=(
        # NVMe PCI test device
        --blockdev driver=file,node-name=nvme-zoned-file,filename="${nvme_data_file}"
        --device nvme,serial=zoned,addr=07.0,zoned.zasl=5
        --device nvme-ns,drive=nvme-zoned-file,zoned=true,zoned.zone_size=64M,zoned.max_open=16,zoned.max_active=32
    )

fi

qemu-system-x86_64 "${qemu_args[@]}" </dev/null &
trap 'kill -TERM %1; wait; rm -fr "${temp_dir}"' EXIT

# wait for guest to be ready

__log_info 'Booting guest...'

# -o PreferredAuthentications=password \
function __ssh() {
    sshpass \
        -p root \
        ssh \
        -tt \
        -o ConnectTimeout=60 \
        -o LogLevel=ERROR \
        -o PubkeyAuthentication=no \
        -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null \
        -p 12345 \
        root@localhost \
        "$@"
}

function __ssh_batch() {
    __ssh "${@@Q}" </dev/null
}

sleep 3  # avoid "connection refused" errors
__ssh_batch true

# run tests in guest

set +o errexit
(
    set -o errexit -o pipefail -o nounset

    function __tests_failed() {
        exit_code="$?"
        __ssh_batch cat build/meson-logs/testlog.txt
        exit "${exit_code}"
    }

    function __ng_dev_from_pci() {
        __ssh_batch readlink "/dev/disk/by-path/pci-$1-nvme-1" | sed 's%../../nvme%/dev/ng%'
    }

    __log_info 'Compiling libblkio in guest...'
    __ssh_batch meson setup --buildtype=debug build libblkio
    __ssh_batch meson compile -C build

    if __suite_is_enabled io_uring; then

        __log_info 'Configuring io_uring test device...'
        __ssh_batch driverctl unset-override 0000:00:05.0

        __log_info 'Running io_uring tests against a block device in guest...'
        __ssh_batch meson test -C build --suite io_uring \
            --test-args '--path /dev/vdb' \
            || __tests_failed

        __log_info 'Running io_uring tests against a regular file in guest...'
        __ssh_batch meson test -C build --suite io_uring \
            || __tests_failed

    fi

    if __suite_is_enabled nvme-io_uring; then

        __log_info 'Configuring nvme-io_uring test device...'
        __ssh_batch driverctl unset-override 0000:00:06.0

        device=$(__ng_dev_from_pci '0000:00:06.0')

        __log_info 'Running nvme-io_uring tests in guest...'
        __ssh_batch meson test -C build --suite nvme-io_uring \
            --test-args "--path ${device}" \
            || __tests_failed

    fi

    if __suite_is_enabled nvme-io_uring-zoned; then

        nr_zones=$((128/64))

        __log_info 'Configuring nvme-io_uring-zoned test device...'
        __ssh_batch driverctl unset-override 0000:00:07.0

        zoned_device=$(__ng_dev_from_pci '0000:00:07.0')

        __log_info 'Running nvme-io_uring-zoned tests in guest...'
        __ssh_batch meson test -C build --verbose --suite nvme-io_uring-zoned \
            --test-args "--path ${zoned_device} zoned 2 zone-size 67108864 nr-zones "${nr_zones}" append-support true
                    max-open-zones 16 max-active-zones 32 zone-append-max-bytes 131072" \
            || __tests_failed

    fi

    if __suite_is_enabled virtio-blk-vfio-pci; then

        __log_info 'Configuring virtio-blk-vfio-pci test device...'
        __ssh_batch driverctl set-override 0000:00:05.0 vfio-pci

        __log_info 'Running virtio-blk-vfio-pci tests in guest...'
        __ssh_batch meson test -C build --suite virtio-blk-vfio-pci \
            --test-args '--path /sys/bus/pci/devices/0000:00:05.0' \
            || __tests_failed

        __log_info 'Configuring virtio-blk-vfio-pci (packed vq) test device...'
        __ssh_batch driverctl set-override 0000:00:08.0 vfio-pci

        __log_info 'Running virtio-blk-vfio-pci (packed vq) tests in guest...'
        __ssh_batch meson test -C build --suite virtio-blk-vfio-pci \
            --test-args '--path /sys/bus/pci/devices/0000:00:08.0' \
            || __tests_failed

    fi

    if __suite_is_enabled virtio-blk-vhost-user; then

        __log_info 'Configuring virtio-blk-vhost-user test device...'
        __ssh_batch driverctl unset-override 0000:00:05.0

        __log_info 'Running virtio-blk-vhost-user tests in guest...'
        __ssh_batch /bin/bash -c '
            set -e
            trap "kill -TERM %1; wait" EXIT
            qemu-storage-daemon \
                --blockdev driver=host_device,node-name=dev,filename=/dev/vdb,cache.direct=on \
                --export type=vhost-user-blk,id=export,node-name=dev,writable=on,num-queues=4,addr.type=unix,addr.path=/root/vhost-user-blk.sock &
            meson test -C build --suite virtio-blk-vhost-user \
                --test-args "--path /root/vhost-user-blk.sock"
            ' || __tests_failed

    fi

    if __suite_is_enabled virtio-blk-vhost-vdpa; then

        __log_info 'Configuring vDPA block device simulator...'
        __ssh_batch modprobe -a vhost-vdpa vdpa-sim-blk
        __ssh_batch vdpa dev add mgmtdev vdpasim_blk name blk0

        __log_info 'Running virtio-blk-vhost-vdpa tests in guest...'
        __ssh_batch meson test -C build --suite virtio-blk-vhost-vdpa \
            --test-args '--path /dev/vhost-vdpa-0' \
            || __tests_failed

    fi
)
exit_code="$?"
set -o errexit

function __terminate_guest() {
    __log_info 'Terminating guest...'
    kill -TERM %1
    wait
    trap 'rm -fr "${temp_dir}"' EXIT
}

if (( exit_code == 0 )); then
    __terminate_guest
    __log_success 'All tests passed!'
else
    __log_failure 'Something failed, starting interactive shell...'
    __ssh
    __terminate_guest
    exit "${exit_code}"
fi
