# localblocksync
Sync of local block file, only write change and not all like dd, better for SSD life, less write.

Written in RUST for speed and less memory overhead. (tested at 400MB/s over SATA and USB3 drive)

Build it:
>cargo b --release

Quickusage:
>target/release/localblocksync -t <src_path> <dst_path>

In example:
>sudo target/release/localblocksync -t /dev/sda1 /media/my_username/mydrive/backup-sda1.img

For help usage:
>target/release/localblocksync -h
