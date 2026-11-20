# Performance tuning

This document shows how to tune a host so that DPDS forwards at line rate. It describes one
reference setup, a KVM guest with two passed-through 100 Gb/s NICs, layer by layer, with the
files and commands that make each layer permanent.

DPDS applies none of this. Neither `dpds.sh`, the controller nor the web frontend writes a kernel,
sysfs, NIC or firmware setting. They only report what they find (see [Verify](#verify)).

## Throughput ceiling: one core per link

Each link runs on one worker core with one RX and one TX queue. 100 Gb/s of 1518 B frames
(8.1 Mpps) fits one core. 100 Gb/s of 64 B frames (148.8 Mpps) does not, whatever the tuning.

## Reference setup

| Part        | Setting                                                                                    |
| ----------- | ------------------------------------------------------------------------------------------ |
| Hypervisor  | Proxmox VE (KVM), one AMD EPYC 7543: 32 cores, SMT on, one NUMA node, 4 cores per L3 cache  |
| NICs        | Two NVIDIA ConnectX-6 Dx (2 × 100 GbE), firmware 22.49.1014                                 |
| Passthrough | Port 0 of each card, passed through as a whole PCI device. Port 1 is unused.               |
| Guest       | Ubuntu 26.04, 8 vCPUs pinned to host cores 4–11 (two whole L3 domains)                      |
| Guest cores | DPDS workers on vCPUs 1–6. The guest OS on vCPUs 0 and 7, the DPDS main lcore on vCPU 0.    |
| Memory      | 64 GiB VM memory in 1 GiB host hugepages. Inside it, 44 × 1 GiB guest hugepages for DPDK.   |

With all settings below, DPDS forwards **100 Gb/s of 1518 B frames per link on one core**.

The settings were applied as a whole and reached line rate together. Most were never measured
one by one, so the list is what was applied, not what each item gains.

For another host, change the PCI addresses, the core lists and the hugepage counts. Use one port
per physical card when DPDS uses two ports.

The tuning has four layers:

1. [Kernel command line](#1-kernel-command-line) of the machine DPDS runs on
2. [Runtime settings at every boot](#2-runtime-settings-at-every-boot) of that machine
3. [NIC firmware](#3-nic-firmware-mlx5)
4. [Hypervisor](#4-hypervisor-kvm--proxmox-ve), only when DPDS runs in a VM

## 1. Kernel command line

**What:** isolate the worker cores from the OS and reserve 1 GiB hugepages. This applies to a VM
guest and to bare metal alike.

**Why:** `nohz_full` stops the periodic tick on the worker cores. `rcu_nocbs` and `rcu_nocb_poll`
move RCU callback work off them. Combined with real-time priority (see [Don't](#dont)), a worker
without them once stalled RCU and made the host unreachable. Without `managed_irq` and
`irqaffinity`, the NIC's own kernel interrupts can land on the worker cores.

The reference guest uses workers `1-6` and housekeeping cores `0,7`:

```
default_hugepagesz=1G hugepagesz=1G hugepages=44 isolcpus=managed_irq,domain,1-6 nohz_full=1-6 rcu_nocbs=1-6 rcu_nocb_poll irqaffinity=0,7 audit=0 iommu=pt
```

| Parameter                               | Effect                                                            |
| --------------------------------------- | ----------------------------------------------------------------- |
| `default_hugepagesz=1G hugepagesz=1G hugepages=44` | 44 × 1 GiB hugepages for the mbuf pools, reserved at boot |
| `isolcpus=managed_irq,domain,1-6`       | No load balancing onto 1–6, managed interrupts kept off where possible |
| `nohz_full=1-6`                         | No periodic tick on the workers                                   |
| `rcu_nocbs=1-6 rcu_nocb_poll`           | RCU callbacks run on the housekeeping cores, by polling           |
| `irqaffinity=0,7`                       | Other interrupts default to the housekeeping cores                 |
| `audit=0`                               | No kernel audit records                                           |
| `iommu=pt`                              | Devices the kernel owns skip IOMMU translation                    |

**Permanent:** on Ubuntu, write it to `/etc/default/grub.d/99-dpdk.cfg`, then run
`sudo update-grub` and reboot:

```
GRUB_CMDLINE_LINUX_DEFAULT="default_hugepagesz=1G hugepagesz=1G hugepages=44 isolcpus=managed_irq,domain,1-6 nohz_full=1-6 rcu_nocbs=1-6 rcu_nocb_poll irqaffinity=0,7 audit=0 iommu=pt"
```

This replaces the value set by `/etc/default/grub` and by earlier `grub.d` files, for example the
`console=` settings of a cloud image. Carry those over into the line.

The controller picks the lowest core that is not isolated as the main lcore (vCPU 0 here) and
one isolated core per link as its worker. Keep the workers off the cores the guest OS runs on.

## 2. Runtime settings at every boot

**What:** four NIC and interrupt settings and two sysctls.

| Setting                    | Command                                                                         |
| -------------------------- | ------------------------------------------------------------------------------- |
| PCIe MaxReadReq 4096 B     | `setpci -s <PCI> CAP_EXP+8.w=5000:7000` (default 512 B)                         |
| Flow control off           | `ethtool -A <netdev> rx off tx off`                                             |
| Whole port receive buffer  | `dcb buffer set dev <netdev> buffer-size all:0 <idx>:<total>` (mlx5)            |
| Interrupts off the workers | `IRQBALANCE_BANNED_CPULIST=<workers> irqbalance --oneshot`, daemon disabled     |
| Memory                     | `vm.zone_reclaim_mode = 0`, `vm.swappiness = 0`                                 |

For the port buffer, `<idx>` is the buffer that priority 0 maps to and `<total>` is the
`total-size`, both read from `dcb buffer show dev <netdev>`. Read the layout live every time, since
it differs between VM starts. The irqbalance daemon stays disabled so that it cannot move the
interrupts back.

**Why a service:** none of the first four survives a reboot. In a guest, every VM start also
resets the port buffer: the passthrough reset restores the default (ConnectX firmware known issue
1895917). So a oneshot systemd service applies them at every boot.

The script `/usr/local/sbin/dpds-tune.sh`. Set `PCI_ADDRESSES` to the guest's PCI addresses of the
DPDS ports (`lspci -D`). Every step runs even if an earlier one fails. It exits 1 if any failed.

```bash
#!/bin/bash
PCI_ADDRESSES="<PCI0> <PCI1>"

rc=0
fail() { echo "dpds-tune: $*" >&2; rc=1; }

for pci in $PCI_ADDRESSES; do
    # PCIe MaxReadReq 4096 B (Device Control bits 14:12).
    setpci -s "$pci" CAP_EXP+8.w=5000:7000 || fail "$pci: setpci failed"

    netdev="$(ls "/sys/bus/pci/devices/$pci/net" 2>/dev/null | head -n 1)"
    [ -n "$netdev" ] || { fail "$pci: no network interface"; continue; }

    # Flow control off. `ethtool -A` fails when nothing changes, so the result is read back.
    ethtool -A "$netdev" rx off tx off 2>/dev/null
    [ "$(ethtool -a "$netdev" | grep -cE '^(RX|TX):[[:space:]]+off')" = 2 ] \
        || fail "$netdev: flow control is not off"

    # The whole port receive buffer for the buffer that priority 0 uses, layout read live.
    show="$(dcb buffer show dev "$netdev" 2>/dev/null)"
    idx="$(printf '%s\n' "$show" | awk '/^prio-buffer/{for(i=2;i<=NF;i++){split($i,a,":"); if(a[1]==0) print a[2]}}')"
    total="$(printf '%s\n' "$show" | awk '/^total-size/{print $2}' | sed 's/b$//')"
    if [ -n "$idx" ] && [ -n "$total" ]; then
        dcb buffer set dev "$netdev" buffer-size all:0 "$idx:$total" || fail "$netdev: dcb buffer set failed"
    else
        fail "$netdev: cannot read the port buffer layout"
    fi
done

# Interrupts off the isolated cores, once. irqbalance may ignore the older
# IRQBALANCE_BANNED_CPUS mask, so the cpulist is used.
isolated="$(cat /sys/devices/system/cpu/isolated 2>/dev/null)"
if [ -n "$isolated" ] && command -v irqbalance >/dev/null 2>&1; then
    IRQBALANCE_BANNED_CPULIST=$isolated irqbalance --oneshot || fail "irqbalance --oneshot failed"
fi

exit $rc
```

The unit `/etc/systemd/system/dpds-tune.service`:

```ini
[Unit]
Description=NIC and interrupt tuning for DPDS (runtime settings that do not survive a reboot)
Wants=systemd-udev-settle.service
After=systemd-udev-settle.service
Before=dpds-controller.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/dpds-tune.sh

[Install]
WantedBy=multi-user.target
```

The sysctl file `/etc/sysctl.d/90-dpds-tune.conf`:

```
vm.zone_reclaim_mode = 0
vm.swappiness = 0
```

The script needs `setpci` (pciutils), `ethtool`, `dcb` (iproute2 5.11 or later) and `irqbalance`,
installed but with its daemon disabled. Install and enable the files (the restart re-runs the
script after a re-install):

```bash
sudo apt install pciutils ethtool iproute2 irqbalance
sudo install -m 0755 dpds-tune.sh /usr/local/sbin/dpds-tune.sh
sudo install -m 0644 dpds-tune.service /etc/systemd/system/dpds-tune.service
sudo install -m 0644 90-dpds-tune.conf /etc/sysctl.d/90-dpds-tune.conf
sudo sysctl --system
sudo systemctl disable --now irqbalance
sudo systemctl daemon-reload
sudo systemctl enable dpds-tune.service && sudo systemctl restart dpds-tune.service
```

## 3. NIC firmware (mlx5)

**What:** three firmware values, set with `mlxconfig` (or `mstconfig`):

```bash
mlxconfig -d <PCI> -y set ADVANCED_PCI_SETTINGS=1
mlxconfig -d <PCI> -y set MAX_ACC_OUT_READ=256 CQE_COMPRESSION=1
```

**Why:** `CQE_COMPRESSION=1` (aggressive) means fewer RX completion writes per packet.
`MAX_ACC_OUT_READ` sets how many bytes of PCIe read requests may be outstanding, which bounds the
TX DMA throughput. The firmware clamps it to the device maximum: 44 on a ConnectX-5, 58 on a
ConnectX-6 Dx. On a ConnectX-5, whose default is 0, this value alone was worth 3 to 4 Gb/s of
zero-loss throughput. The ConnectX-6 Dx defaults to 32, so expect a smaller gain there.

**Permanent:** the values are stored in the card's NVRAM and survive reboots. They only become
active after a firmware reset, which a guest cannot do. The safe path is a reboot of the
hypervisor. Without one, reset each card on the hypervisor with the VM stopped. `<PCI>` is the
hypervisor's address of the passed-through port, which stays bound to vfio-pci. The card's
unused second port must be unbound from mlx5_core (the boot-time script of section 4 does that).
The reset hits the whole card. This form worked with mstflint 4.37.0:

```bash
qm stop <vmid>
mstfwreset -d <PCI> reset --level 3 --sync 0 --skip_driver -y
```

Check the values with `mlxconfig -d <PCI> -e query` and the running firmware with
`mstflint -d <PCI> query | grep -E 'FW Version|PSID'`. Keep the firmware current (22.49.1014 in
the reference setup).

## 4. Hypervisor (KVM / Proxmox VE)

The guest's isolation is worth little while the hypervisor still schedules, ticks and handles
interrupts on the same physical cores. In the reference setup the guest's vCPU n runs on host core
4+n. Cores 36–43 are the SMT siblings of 4–11. Everything else (`0-3,12-35,44-63`) is housekeeping.

| Setting                                  | Why                                                                   |
| ---------------------------------------- | --------------------------------------------------------------------- |
| One host core per vCPU, whole L3 domains | A vCPU never shares a core or moves between L3 caches                 |
| Isolate those cores and their siblings   | Runtime confinement (cgroups, IRQ masks) leaves the host tick and timers |
| SMT siblings offline, governor `performance` | No sibling competes for the core, no frequency ramp-up            |
| All VM memory in 1 GiB host hugepages    | Fewer IOMMU translation misses on NIC DMA                             |
| Ballooning off, CPU type `host`          | vfio pins all guest memory anyway. The guest sees the real CPU features. |
| Unused ports of a passed-through card unbound | The host driver does not use the card                            |

**Kernel command line.** On a Proxmox host with systemd-boot, append to the single line in
`/etc/kernel/cmdline`, then run `proxmox-boot-tool refresh` and reboot:

```
default_hugepagesz=1G hugepagesz=1G hugepages=64 isolcpus=managed_irq,domain,4-11,36-43 nohz_full=4-11,36-43 rcu_nocbs=4-11,36-43 irqaffinity=0-3,12-35,44-63
```

**Boot-time script** `/usr/local/bin/dpds-vm-host-tune.sh`, for what the command line cannot do:

```bash
#!/bin/bash
for c in $(seq 36 43); do echo 0 > /sys/devices/system/cpu/cpu$c/online; done
for c in $(seq 4 11); do echo performance > /sys/devices/system/cpu/cpu$c/cpufreq/scaling_governor; done
for f in <PCI of each unused port>; do
  [ -e /sys/bus/pci/drivers/mlx5_core/$f ] && echo $f > /sys/bus/pci/drivers/mlx5_core/unbind
done
exit 0
```

Its unit `/etc/systemd/system/dpds-vm-host-tune.service`, enabled with
`systemctl enable dpds-vm-host-tune.service`:

```ini
[Unit]
Description=Host tuning for the DPDS VM
Wants=systemd-udev-settle.service
After=systemd-udev-settle.service
Before=pve-guests.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/bin/dpds-vm-host-tune.sh

[Install]
WantedBy=multi-user.target
```

**Per-vCPU pinning hookscript** `/var/lib/vz/snippets/dpds-vm-pin.sh`. Proxmox runs it after
every VM start. QEMU names its vCPU threads `CPU n/KVM`. Every other QEMU thread goes to the
housekeeping cores.

```bash
#!/bin/bash
# Proxmox hookscript: $1 = vmid, $2 = phase
[ "$2" = post-start ] || exit 0
pid=$(cat /var/run/qemu-server/$1.pid)
for t in /proc/$pid/task/*; do
  tid=${t##*/}
  if [[ $(cat $t/comm) =~ ^CPU\ ([0-9]+)/KVM$ ]]; then
    taskset -cp $((4 + BASH_REMATCH[1])) $tid
  else
    taskset -cp 0-3,12-35,44-63 $tid
  fi
done
```

It needs `snippets` in the content list of the `local` storage (`/etc/pve/storage.cfg`).

**VM config.** The reference VM has `cores: 8`, `sockets: 1`, `cpu: host`, `memory: 65536`, and
port 0 of each card as `hostpci0` and `hostpci1`. Attach the rest with:

```bash
chmod +x /usr/local/bin/dpds-vm-host-tune.sh /var/lib/vz/snippets/dpds-vm-pin.sh
qm set <vmid> --hookscript local:snippets/dpds-vm-pin.sh --hugepages 1024 --keephugepages 1 --numa 1 --balloon 0
qm set <vmid> --delete affinity
```

**Do not set the VM's CPU `affinity`** to the isolated cores. `isolcpus` turns load balancing off,
so all vCPU threads could stay stacked on the first core. The hookscript does all pinning.

Be aware of:

- The isolated cores are off-limits to everything else. Other VMs and host services never run
  there unless pinned explicitly.
- The 64 GiB of hugepages are reserved at boot, whether the VM runs or not.
- If irqbalance runs on the hypervisor, set `IRQBALANCE_BANNED_CPULIST=4-11,36-43` in
  `/etc/default/irqbalance`.

**ZFS root: use `lz4`, not `zstd`.** ZFS writes changed data out every 5 s, which the host
timer rounds to a 5.12 s grid. With `zstd`, compressing and checksumming a large write-out stalled
the NICs' DMA writes into RAM, and the NIC port buffers overflowed. The losses came in batches on
the 5.12 s grid, not in every period. With `lz4`, a 2.5 h run at line rate lost no packet.

```bash
zfs set compression=lz4 rpool
zfs get -r compression rpool    # every dataset lz4
```

A dataset with a locally set `zstd` does not follow the pool, so set it there too. Data written
before the change stays `zstd`-compressed until it is rewritten, which is harmless.

**BIOS.** The reference host uses settings along the lines of AMD's EPYC 7003 "NIC Throughput
Intensive" profile. Packet loss fell sharply after this change. Other changes happened at the same
time, so its effect alone was not measured.

| Setting            | Value                             |
| ------------------ | --------------------------------- |
| APBDIS             | 1                                 |
| Fixed SOC P-State  | P0                                |
| Preferred I/O      | Enhanced Preferred I/O, all NBIOs |
| LCLK               | Auto                              |
| Determinism Slider | Power                             |
| Core C-states      | C0/C1 only                        |
| DF C-states        | Disabled                          |

## Don't

- **`chrt` / `SCHED_FIFO`:** in earlier tests this made the host unreachable or broke forwarding,
  with no gain.
- **`perf record` on a busy-polling core in a VM:** it lowered cycles and the measured throughput.
  Use `perf stat`.
- **Stack mempool ops** (`--mbuf-pool-ops-name=stack`): earlier tests lost millions of packets to
  `imissed`.

## Verify

`./dpds.sh check` and the Host hints page of the web frontend compare the machine DPDS runs on with
these settings and show the commands to fix a gap. They see the guest only, never the hypervisor
above it. A few direct checks on the guest:

```bash
cat /proc/cmdline /sys/devices/system/cpu/isolated
sudo lspci -s <PCI> -vv | grep -o 'MaxReadReq [0-9]* bytes'   # 4096
ethtool -a <netdev>                                            # RX: off, TX: off
dcb buffer show dev <netdev>                                   # total-size in the buffer of priority 0
systemctl is-active dpds-tune irqbalance                       # active, inactive
```
