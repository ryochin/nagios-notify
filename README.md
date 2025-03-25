Nagios Notify
=============

Deployment
----------

### Setup

```sh
mkdir -p /app/nagios-notify
install -d -m 0775 /app/nagios-notify/log

touch /app/nagios-notify/config.yml
chmod 640 /app/nagios-notify/config.yml
nano /app/nagios-notify/config.yml
```

### Install

```sh
# asset
install -c -m 0644 template.txt /app/nagios-notify/

# binary
cargo build --release
install -c -m 0755 target/release/nagios-notify /app/nagios-notify/notify

# misc
install -c -m 0644 logrotate.d/nagios-notify /etc/logrotate.d/
```

