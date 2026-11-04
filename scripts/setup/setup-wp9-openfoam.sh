#!/bin/bash
# Approved project distro setup; no solver implementation source is inspected.
set -euo pipefail
mode=${1:-inspect}
[[ "$mode" == inspect || "$mode" == install ]]
[[ $(id -u) == 0 ]]
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 24.04 ]]
[[ $(dpkg --print-architecture) == amd64 ]]
# Second argument: the Windows tools folder's openfoam evidence directory, as a WSL path.
evidence=${2:?usage: setup-wp9-openfoam.sh inspect|install /mnt/<drive>/.../tools/wsl/openfoam}
[[ "$evidence" == /mnt/[a-z]/* ]]
mkdir -p "$evidence"
export DEBIAN_FRONTEND=noninteractive
apt_args=(-o Acquire::Retries=2 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 -o APT::Update::Error-Mode=any)
if [[ "$mode" == inspect ]]; then
    apt-get "${apt_args[@]}" update
    apt-get "${apt_args[@]}" install -y --no-install-recommends ca-certificates curl gnupg
    curl -fL --connect-timeout 20 --max-time 120 --retry 2 \
        https://dl.openfoam.com/pubkey.gpg -o "$evidence/vendor-key.download"
    sha256sum "$evidence/vendor-key.download" > "$evidence/vendor-key.sha256"
    gpg --batch --show-keys --with-colons "$evidence/vendor-key.download" > "$evidence/vendor-key.identity"
    gpg --batch --yes --dearmor --output /usr/share/keyrings/vernon-openfoam.gpg "$evidence/vendor-key.download"
    printf '%s\n' 'deb [arch=amd64 signed-by=/usr/share/keyrings/vernon-openfoam.gpg] https://dl.openfoam.com/repos/deb noble main' \
        > /etc/apt/sources.list.d/vernon-openfoam.list
    apt-get "${apt_args[@]}" update
    apt-cache policy openfoam2606 > "$evidence/package-policy.txt"
    apt-cache show --no-all-versions openfoam2606 > "$evidence/package-metadata.txt"
    apt-get -s install --no-install-recommends openfoam2606 > "$evidence/install-simulation.txt"
    candidate=$(apt-cache policy openfoam2606 | awk '/Candidate:/ {print $2}')
    [[ -n "$candidate" && "$candidate" != '(none)' ]]
    printf '%s\n' "$candidate" > "$evidence/candidate.txt"
    echo "WP9_APT_METADATA_READY candidate=$candidate"
else
    # Install exactly the candidate reviewed in the preceding signed-metadata phase.
    candidate=$(cat "$evidence/candidate.txt")
    current=$(apt-cache policy openfoam2606 | awk '/Candidate:/ {print $2}')
    [[ "$candidate" == "$current" && "$candidate" == 2606* ]]
    apt-get "${apt_args[@]}" install -y --no-install-recommends "openfoam2606=$candidate"
    dpkg-query -W -f='${Package}\t${Version}\t${Status}\n' 'openfoam2606*' > "$evidence/installed-packages.txt"
    dpkg -L openfoam2606 > "$evidence/runtime-files.txt"
    # Unprivileged project user the CFD runner uses; useradd leaves its password locked.
    id -u vernon >/dev/null 2>&1 || useradd --create-home --shell /bin/bash vernon
    [[ $(id -u vernon) != 0 ]]
    echo "WP9_OPENFOAM_PACKAGE_INSTALLED version=$candidate"
fi
