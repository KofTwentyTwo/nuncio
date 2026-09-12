FROM debian:12@sha256:813017f3d62be4b5891a7acca6a01bdcd4b8513daa81b1ab99d3a50385b26931
RUN apt-get update && apt-get install --no-install-recommends -y python3 iptables iproute2 sudo \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1000 runner \
    && printf 'runner ALL=(root) NOPASSWD: /usr/sbin/iptables, /usr/sbin/ip6tables, /usr/sbin/ip, /usr/bin/python3 /egress_cgroup.py *\n' > /etc/sudoers.d/egress \
    && chmod 0440 /etc/sudoers.d/egress
USER runner
WORKDIR /tmp
