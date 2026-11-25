#!/usr/bin/env bash
# Praxi host provisioning helper. Uploaded by the desktop app over SSH next to
# install-linux.sh and run with a FIXED subcommand. It never evals input:
# parameters arrive on stdin as `key=value` lines and are validated here again.
#
#   preflight   read-only facts as key=value lines (no root needed)
#   status      service/desktop state as key=value lines (no root needed)
#   install-gh  install GitHub CLI >= 2.80 (needed by install-linux.sh's provenance check)
#   snapshot    tar the data dir into /var/backups/praxi before touching an existing install
#   install     run install-linux.sh --version <tag> --host-mode [--with-desktop-session]
#   invite|devices|screen|revoke   `bot host ...` as the unprivileged praxi user
#   uninstall|purge                install-linux.sh --uninstall [--purge]
#   packs-facts|packs-plan         capability packs (packs.txt): facts and dry run, no root needed
#   pack-install|pack-remove       install or remove one catalog pack (root, apt only)
set -euo pipefail

STAGE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX=/opt/praxi
DATA=/var/lib/praxi
ENVF=/etc/praxi/praxi.env
UNIT=/etc/systemd/system/praxi.service
MIN_GH=2.80.0

die() { printf '[praxi-provision] error: %s\n' "$*" >&2; exit 1; }
log() { printf '[praxi-provision] %s\n' "$*"; }
need_root() { [[ "${EUID:-$(id -u)}" -eq 0 ]] || die "this step must run as root (passwordless sudo is required)"; }

declare -A P=()
read_params() {
  local line k v
  while IFS= read -r line || [[ -n "${line}" ]]; do
    [[ -n "${line}" ]] || continue
    k="${line%%=*}"; v="${line#*=}"
    [[ "${k}" =~ ^[a-z_]{1,32}$ ]] || die "bad parameter name"
    [[ "${v}" =~ ^[A-Za-z0-9._:-]{0,160}$ ]] || die "bad value for ${k}"
    P["${k}"]="${v}"
  done
}

# kv name value: print one sanitised fact line.
kv() { printf '%s=%s\n' "$1" "$(printf '%s' "${2:-}" | tr -cd 'A-Za-z0-9._:,+-' | cut -c1-80)"; }

version_ge() { [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" == "$2" ]]; }

gh_version() { gh --version 2>/dev/null | awk 'NR==1 {print $3}'; }

env_port() {
  local p=47821
  if [[ -r "${ENVF}" ]] && grep -q '^BOT_PORT=' "${ENVF}"; then
    p="$(grep '^BOT_PORT=' "${ENVF}" | tail -n1 | cut -d= -f2- | tr -d '[:space:]')"
  fi
  [[ "${p}" =~ ^[0-9]+$ && "${p}" -ge 1 && "${p}" -le 65535 ]] || die "invalid BOT_PORT"
  printf '%s' "${p}"
}

# Read the manager's effective command after unit and drop-in merging.
unit_hostmode() {
  local exec_start executable args
  exec_start="$(systemctl show --property=ExecStart --value praxi.service 2>/dev/null)" || return 1
  [[ "${exec_start}" == '{ path='*' ; argv[]='* ]] || return 1
  executable="${exec_start#'{ path='}"
  executable="${executable%%' ; argv[]='*}"
  [[ "${executable##*/}" == bot ]] || return 1
  args="${exec_start#*' ; argv[]='}"
  args="${args%%' ; ignore_errors='*}"
  [[ "${args}" =~ [[:space:]]serve[[:space:]]+([^[:space:]]+[[:space:]]+)*--host([[:space:]]|$) ]]
}

cmd_preflight() {
  kv uid "$(id -u)"
  ( . /etc/os-release 2>/dev/null || true
    kv os_id "${ID:-}"; kv os_version "${VERSION_ID:-}"; kv os_like "${ID_LIKE:-}"; kv os_name "${PRETTY_NAME:-}" )
  kv arch "$(uname -m)"
  if [[ -d /run/systemd/system ]]; then kv systemd 1; else kv systemd 0; fi
  if [[ "$(id -u)" -eq 0 ]] || sudo -n true >/dev/null 2>&1; then kv sudo_n 1; else kv sudo_n 0; fi
  kv ram_mb "$(awk '/^MemTotal:/ {print int($2/1024)}' /proc/meminfo 2>/dev/null || echo 0)"
  kv disk_mb "$(df -Pm / 2>/dev/null | awk 'NR==2 {print $4}' || echo 0)"
  if command -v curl >/dev/null 2>&1; then
    kv curl 1
    if curl -fsI --max-time 12 --proto '=https' --tlsv1.2 https://github.com >/dev/null 2>&1; then kv https 1; else kv https 0; fi
  else
    kv curl 0; kv https 0
  fi
  kv gh_version "$(gh_version || true)"
  kv praxi_version "$("${PREFIX}/bin/bot" --version 2>/dev/null | awk '{print $2}' || true)"
  kv praxi_build "$("${PREFIX}/bin/bot" --version 2>/dev/null | awk '{print $3}' | tr -cd 'A-Za-z0-9.+_-' || true)"
  if [[ -f "${UNIT}" ]]; then kv unit_present 1; else kv unit_present 0; fi
  if unit_hostmode; then kv unit_hostmode 1; else kv unit_hostmode 0; fi
  kv unit_active "$(systemctl is-active praxi.service 2>/dev/null || true)"
  if [[ -d "${DATA}" ]]; then kv data_dir 1; else kv data_dir 0; fi
  local port=47821
  if [[ "$(id -u)" -eq 0 ]]; then port="$(env_port)"; elif sudo -n true >/dev/null 2>&1 && sudo -n test -r "${ENVF}" 2>/dev/null; then
    port="$(sudo -n grep '^BOT_PORT=' "${ENVF}" 2>/dev/null | tail -n1 | cut -d= -f2- | tr -cd '0-9')"; port="${port:-47821}"
  fi
  kv port "${port}"
  if command -v ss >/dev/null 2>&1 && ss -ltnH 2>/dev/null | awk '{print $4}' | grep -Eq "[:.]${port}\$"; then kv port_busy 1; else kv port_busy 0; fi
}

# 1 when a non-snap Chrome/Chromium that Praxi can launch is installed.
browser_present() {
  local c
  for c in /opt/praxi-browser/chrome /usr/bin/google-chrome /usr/bin/google-chrome-stable /usr/bin/chromium /usr/bin/chromium-browser; do
    [[ -x "${c}" ]] || continue
    [[ "$(readlink -f "${c}")" != /snap/* ]] || continue
    ! head -c 8192 "${c}" | grep -aqE "snap run|/snap/" || continue
    return 0
  done
  return 1
}

cmd_status() {
  kv version "$("${PREFIX}/bin/bot" --version 2>/dev/null | awk '{print $2}' || true)"
  kv unit_active "$(systemctl is-active praxi.service 2>/dev/null || true)"
  if unit_hostmode; then kv unit_hostmode 1; else kv unit_hostmode 0; fi
  kv xvfb "$(systemctl is-active praxi-xvfb.service 2>/dev/null || true)"
  kv desktop "$(systemctl is-active praxi-desktop.service 2>/dev/null || true)"
  if browser_present; then kv browser 1; else kv browser 0; fi
}

# Keep needrestart from restarting the live installation during package setup.
apt_without_restarts() {
  DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 apt-get "$@"
}

cmd_install_gh() {
  need_root
  local have; have="$(gh_version || true)"
  if [[ -n "${have}" ]] && version_ge "${have}" "${MIN_GH}"; then log "gh ${have} already installed"; return 0; fi
  if command -v apt-get >/dev/null 2>&1; then
    apt_without_restarts update -qq
    apt_without_restarts install -y -qq curl ca-certificates
    install -d -m 755 /etc/apt/keyrings
    curl -fsSL --proto '=https' --tlsv1.2 https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      -o /etc/apt/keyrings/githubcli-archive-keyring.gpg
    chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list
    apt_without_restarts update -qq
    apt_without_restarts install -y -qq gh
  elif command -v dnf >/dev/null 2>&1; then
    dnf install -y -q curl 'dnf-command(config-manager)' || true
    dnf config-manager --add-repo https://cli.github.com/packages/rpm/gh-cli.repo >/dev/null 2>&1 \
      || dnf config-manager addrepo --from-repofile=https://cli.github.com/packages/rpm/gh-cli.repo
    dnf install -y -q gh --repo gh-cli
  else
    die "no supported package manager (apt-get or dnf) to install the GitHub CLI"
  fi
  have="$(gh_version || true)"
  if [[ -z "${have}" ]] || ! version_ge "${have}" "${MIN_GH}"; then die "GitHub CLI >= ${MIN_GH} is required, found '${have:-none}'"; fi
  log "gh ${have} installed"
}

cmd_snapshot() {
  need_root
  [[ -d "${DATA}" ]] || { log "no data dir to snapshot"; return 0; }
  local ts file
  ts="$(date -u +%Y%m%dT%H%M%SZ)"
  install -d -m 700 /var/backups/praxi
  file="/var/backups/praxi/praxi-data-${ts}.tgz"
  ( umask 077; tar -C "$(dirname "${DATA}")" -czf "${file}" "$(basename "${DATA}")" )
  log "snapshot: ${file}"
}

cmd_install() {
  need_root
  read_params
  local version="${P[version]:-}"
  [[ "${version}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.+-][0-9A-Za-z.+-]+)?$ ]] || die "bad or missing version"
  [[ -f "${STAGE}/install-linux.sh" ]] || die "install-linux.sh was not uploaded"
  local args=(--version "${version}" --host-mode)
  if [[ "${P[update]:-0}" == "1" ]]; then
    # In-place update: the installer keeps data, config, display and mode, rolls back by itself if
    # anything fails and keeps the previous program files as /opt/praxi.backup-<timestamp>.
    args+=(--update)
  else
    [[ "${P[desktop]:-1}" != "1" ]] || args+=(--with-desktop-session)
  fi
  DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l NEEDRESTART_SUSPEND=1 bash "${STAGE}/install-linux.sh" "${args[@]}"
}

# `bot` as the unprivileged service user, against the local daemon.
as_praxi() {
  need_root
  local port; port="$(env_port)"
  runuser -u praxi -- env BOT_DATA_DIR="${DATA}" BOT_PORT="${port}" HOME="${DATA}" "${PREFIX}/bin/bot" --json "$@"
}

cmd_invite() { as_praxi host invite --role desktop; }
cmd_devices() { as_praxi host devices; }
cmd_screen() {
  read_params
  [[ "${P[key]:-}" =~ ^[A-Za-z0-9]{1,128}$ ]] || die "bad device key"
  as_praxi host screen "${P[key]}"
}
cmd_screen_revoke() {
  read_params
  [[ "${P[key]:-}" =~ ^[A-Za-z0-9]{1,128}$ ]] || die "bad device key"
  as_praxi host screen "${P[key]}" --revoke
}
cmd_revoke() {
  read_params
  [[ "${P[key]:-}" =~ ^[A-Za-z0-9]{1,128}$ ]] || die "bad device key"
  as_praxi host revoke "${P[key]}"
}
cmd_uninstall() { need_root; bash "${STAGE}/install-linux.sh" --uninstall; }
cmd_purge() { need_root; bash "${STAGE}/install-linux.sh" --uninstall --purge; }

# ---- capability packs ---------------------------------------------------------------------------
# The catalog (packs.txt) is uploaded by the app; its SHA-256 arrives as `catalog=` and must match
# the staged copy, so a changed file in the staging directory is never executed as root.
CATALOG="${STAGE}/packs.txt"
PACK_ID="" PACK_PKGS="" PACK_REMOVABLE=0

catalog_check() {
  [[ -f "${CATALOG}" ]] || die "not-available: the pack catalog was not uploaded"
  local want="${P[catalog]:-}" have
  [[ "${want}" =~ ^[0-9a-f]{64}$ ]] || die "bad catalog checksum"
  have="$(sha256sum "${CATALOG}" | awk '{print $1}')"
  [[ "${have}" == "${want}" ]] || die "catalog-mismatch: the staged pack catalog does not match the app's"
}

# pack_load id: fill PACK_* from the catalog (only a catalog id is ever accepted).
pack_load() {
  local want="$1" id base rem size pkgs tools p
  [[ "${want}" =~ ^[a-z]{2,16}$ ]] || die "bad pack id"
  while IFS='|' read -r id base rem size pkgs tools; do
    [[ "${id}" == "${want}" ]] || continue
    for p in ${pkgs}; do [[ "${p}" =~ ^[a-z0-9][a-z0-9+.-]{1,60}$ ]] || die "bad package name in catalog"; done
    PACK_ID="${id}"; PACK_PKGS="${pkgs}"; PACK_REMOVABLE="${rem}"
    return 0
  done < <(grep -v '^#' "${CATALOG}")
  die "unknown pack"
}

pkg_installed() { dpkg-query -W -f='${db:Status-Abbrev}' "$1" 2>/dev/null | grep -q '^ii'; }
have_apt() { command -v apt-get >/dev/null 2>&1 && command -v dpkg-query >/dev/null 2>&1; }
need_apt() { have_apt || die "not-available: only Debian and Ubuntu servers are supported"; }
has_candidate() { apt-cache policy "$1" 2>/dev/null | grep -q 'Candidate: [^(]'; }

pack_status() {
  local p have=0 total=0
  for p in ${PACK_PKGS}; do
    total=$((total + 1))
    if pkg_installed "${p}"; then have=$((have + 1)); fi
  done
  if [[ "${have}" -eq "${total}" ]]; then printf installed
  elif [[ "${have}" -eq 0 ]]; then printf missing
  else printf partial; fi
}

# What the pack's own packages would drag out with them on `apt-get remove`.
remove_outsiders() {
  local line
  while IFS= read -r line; do
    [[ "${line}" == Remv\ * ]] || continue
    line="${line#Remv }"; line="${line%% *}"
    case " ${PACK_PKGS} " in *" ${line} "*) ;; *) printf '%s' "${line}"; return 0 ;; esac
  done < <(apt-get -s remove "$@" 2>/dev/null || true)
  return 0
}

cmd_packs_facts() {
  [[ -f "${CATALOG}" ]] || die "not-available: the pack catalog was not uploaded"
  local id base rem size pkgs tools t alt found
  if have_apt; then kv apt 1; else kv apt 0; fi
  if [[ "${EUID:-$(id -u)}" -eq 0 ]] || sudo -n true >/dev/null 2>&1; then kv admin 1; else kv admin 0; fi
  kv os_id "$( (. /etc/os-release 2>/dev/null && printf '%s' "${ID:-}") || true)"
  while IFS='|' read -r id base rem size pkgs tools; do
    [[ "${id}" =~ ^[a-z]{2,16}$ ]] || continue
    if have_apt; then PACK_PKGS="${pkgs}"; kv "pack_${id}" "$(pack_status)"; else kv "pack_${id}" missing; fi
    for t in ${tools}; do
      found=0
      for alt in ${t//\// }; do if command -v "${alt}" >/dev/null 2>&1; then found=1; fi; done
      kv "tool_${t%%/*}" "${found}"
    done
  done < <(grep -v '^#' "${CATALOG}")
}

# Dry run: changes nothing. Prints what would be installed or removed so the owner can approve it.
cmd_packs_plan() {
  read_params
  catalog_check; need_apt; pack_load "${P[pack]:-}"
  local action="${P[action]:-install}" p missing="" unavailable="" have="" outsider="" n
  kv pack "${PACK_ID}"; kv action "${action}"
  case "${action}" in
    install)
      for p in ${PACK_PKGS}; do
        if pkg_installed "${p}"; then continue; fi
        missing="${missing:+${missing},}${p}"
        if ! has_candidate "${p}"; then unavailable="${unavailable:+${unavailable},}${p}"; fi
      done
      kv missing "${missing}"; kv unavailable "${unavailable}"
      if [[ -n "${missing}" && -z "${unavailable}" ]]; then
        n="$(apt-get -s install --no-install-recommends ${missing//,/ } 2>/dev/null | grep -c '^Inst ' || true)"
        kv new_packages "${n}"
      fi ;;
    remove)
      if [[ "${PACK_REMOVABLE}" != "1" ]]; then kv blocked shared; return 0; fi
      for p in ${PACK_PKGS}; do
        if pkg_installed "${p}"; then have="${have:+${have} }${p}"; fi
      done
      kv missing "${have// /,}"
      if [[ -n "${have}" ]]; then
        # shellcheck disable=SC2086
        outsider="$(remove_outsiders ${have})"
        if [[ -n "${outsider}" ]]; then kv blocked dependants; fi
      fi ;;
    *) die "bad action" ;;
  esac
}

cmd_pack_install() {
  need_root; read_params
  catalog_check; need_apt; pack_load "${P[pack]:-}"
  local p
  apt_without_restarts update -qq || log "apt update reported a problem; trying with the known package lists"
  for p in ${PACK_PKGS}; do
    has_candidate "${p}" || die "not-available: package ${p} is not offered by this server's software sources"
  done
  # shellcheck disable=SC2086
  apt_without_restarts install -y --no-install-recommends ${PACK_PKGS}
  kv "pack_${PACK_ID}" "$(pack_status)"
}

cmd_pack_remove() {
  need_root; read_params
  catalog_check; need_apt; pack_load "${P[pack]:-}"
  [[ "${PACK_REMOVABLE}" == "1" ]] || die "blocked: this pack holds shared system tools and is not removed"
  local p have="" outsider
  for p in ${PACK_PKGS}; do
    if pkg_installed "${p}"; then have="${have:+${have} }${p}"; fi
  done
  if [[ -z "${have}" ]]; then kv "pack_${PACK_ID}" missing; return 0; fi
  # shellcheck disable=SC2086
  outsider="$(remove_outsiders ${have})"
  [[ -z "${outsider}" ]] || die "blocked: other software on this server depends on it (${outsider})"
  # Plain `remove` only: settings and unrelated packages stay untouched.
  # shellcheck disable=SC2086
  apt_without_restarts remove -y ${have}
  kv "pack_${PACK_ID}" "$(pack_status)"
}

case "${1:-}" in
  preflight) cmd_preflight ;;
  status) cmd_status ;;
  install-gh) cmd_install_gh ;;
  snapshot) cmd_snapshot ;;
  install) cmd_install ;;
  invite) cmd_invite ;;
  devices) cmd_devices ;;
  screen) cmd_screen ;;
  screen-revoke) cmd_screen_revoke ;;
  revoke) cmd_revoke ;;
  uninstall) cmd_uninstall ;;
  purge) cmd_purge ;;
  packs-facts) cmd_packs_facts ;;
  packs-plan) cmd_packs_plan ;;
  pack-install) cmd_pack_install ;;
  pack-remove) cmd_pack_remove ;;
  *) die "unknown subcommand" ;;
esac
