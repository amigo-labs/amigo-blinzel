#!/usr/bin/env bash
# The self-signed code signing certificate for amigo-labs' Mac apps, stored
# as the GitHub secrets MACOS_SIGNING_CERT (the .p12, base64) and
# MACOS_SIGNING_CERT_PASSWORD. Release workflows import it and sign with
# it: not trusted by Gatekeeper, but a stable identity, so macOS keeps
# permissions such as Screen Recording across updates (docs/PLAN.md,
# section 10). One certificate serves every app; create it once. A new one
# makes every Mac ask for those permissions again.
#
# Usage: create-signing-cert.sh [options]
#   --name NAME     certificate name (CN)           default: amigo-labs
#   --dir DIR       folder for the key material     default: ~/amigo-labs-signing
#   --org ORG       organization secrets for ...    default: amigo-labs
#   --repos LIST    ... these repositories (comma-separated, the full list:
#                   it replaces the previous one)   default: amigo-blinzel
#   --repo O/R      a repository secret instead of organization secrets
#   --no-secrets    only create the .p12, set the secrets by hand
#   -h, --help      this text
#
# With DIR/signing.p12 already there it only sets the secrets again (asks
# for its password), e.g. to give one more repository access:
#   create-signing-cert.sh --repos amigo-blinzel,amigo-pincel
# A key.pem and cert.pem in DIR, or in the current folder, are used instead
# of creating new ones. Needs openssl and, for the secrets, gh signed in
# with admin rights on the organization (or the repository). Runs in Git
# Bash on Windows, on macOS and on Linux; the key never goes into a
# repository.

set -euo pipefail

NAME="amigo-labs"
DIR="$HOME/amigo-labs-signing"
ORG="amigo-labs"
REPOS="amigo-blinzel"
REPO=""
SECRETS=1

usage() {
    sed -n '/^# Usage:/,/^# repository\.$/s/^# \{0,1\}//p' "$0"
}

die() {
    echo "error: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --name) NAME="${2:?--name needs a value}"; shift 2 ;;
        --dir) DIR="${2:?--dir needs a value}"; shift 2 ;;
        --org) ORG="${2:?--org needs a value}"; shift 2 ;;
        --repos) REPOS="${2:?--repos needs a value}"; shift 2 ;;
        --repo) REPO="${2:?--repo needs a value}"; shift 2 ;;
        --no-secrets) SECRETS=0; shift ;;
        -h | --help) usage; exit 0 ;;
        *) usage >&2; die "unknown option $1" ;;
    esac
done

command -v openssl >/dev/null || die "openssl not found"
if [ "$SECRETS" = 1 ]; then
    command -v gh >/dev/null || die "gh not found (or use --no-secrets)"
    gh auth status >/dev/null 2>&1 || die "gh is not signed in: gh auth login (or use --no-secrets)"
fi

created=0
[ -d "$DIR" ] || created=1
mkdir -p "$DIR"
DIR="$(cd "$DIR" && pwd)"
d="$DIR"
while :; do
    if [ -e "$d/.git" ]; then
        [ "$created" = 1 ] && rmdir "$DIR"
        die "$DIR is inside the repository $d; pick a folder outside of it"
    fi
    parent="$(dirname "$d")"
    [ "$parent" = "$d" ] && break
    d="$parent"
done

read_password() {
    local again
    while :; do
        read -rs -p "$1" P12_PASS
        echo
        [ -n "$P12_PASS" ] || { echo "the password must not be empty"; continue; }
        [ "$2" = once ] && break
        read -rs -p "again: " again
        echo
        [ "$P12_PASS" = "$again" ] && break
        echo "the passwords differ"
    done
    export P12_PASS
}

if [ -e "$DIR/signing.p12" ]; then
    echo "using the existing $DIR/signing.p12"
    [ "$SECRETS" = 1 ] || die "nothing to do: signing.p12 exists and --no-secrets is set"
    cd "$DIR"
    read_password "password of signing.p12: " once
    openssl pkcs12 -in signing.p12 -passin env:P12_PASS -noout 2>/dev/null \
        || die "signing.p12 does not open with this password"
else
    # A key pair from an earlier attempt.
    if [ ! -e "$DIR/key.pem" ] && [ -e key.pem ] && [ -e cert.pem ]; then
        echo "moving key.pem and cert.pem from $(pwd) to $DIR"
        mv key.pem cert.pem "$DIR/"
    fi
    cd "$DIR"
    # Nobody else may read the key.
    umask 077
    if [ -e key.pem ] && [ -e cert.pem ]; then
        echo "using the existing key.pem and cert.pem in $DIR"
    else
        echo "creating the certificate \"$NAME\" in $DIR"
        # Git Bash would turn "/CN=..." into a Windows path.
        MSYS_NO_PATHCONV=1 openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
            -subj "/CN=$NAME" \
            -addext "keyUsage=critical,digitalSignature" \
            -addext "extendedKeyUsage=critical,codeSigning" \
            -keyout key.pem -out cert.pem 2>&1 | grep -v '^[.+*-]*$' || true
        [ -s cert.pem ] || die "openssl did not create the certificate"
    fi
    openssl x509 -in cert.pem -noout -text | grep -q "Code Signing" \
        || die "cert.pem is not a code signing certificate"
    openssl x509 -in cert.pem -noout -subject -enddate
    read_password "password for signing.p12: " twice
    # 3DES and SHA-1: what macOS' `security import` reads everywhere, and
    # no OpenSSL legacy provider needed (Git Bash has none).
    openssl pkcs12 -export -inkey key.pem -in cert.pem -name "$NAME" \
        -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 \
        -passout env:P12_PASS -out signing.p12
    # The .p12 must open before the key goes.
    openssl pkcs12 -in signing.p12 -passin env:P12_PASS -noout 2>/dev/null \
        || die "signing.p12 does not open with the password"
    rm key.pem
    echo "wrote $DIR/signing.p12 (key.pem deleted: the key lives only in the .p12)"
fi

openssl pkcs12 -in signing.p12 -passin env:P12_PASS -nokeys 2>/dev/null \
    | openssl x509 -noout -fingerprint -sha1

if [ "$SECRETS" = 1 ]; then
    if [ -n "$REPO" ]; then
        target=(-R "$REPO")
        where="repository $REPO"
    else
        target=(--org "$ORG" --visibility selected --repos "$REPOS")
        where="organization $ORG for $REPOS"
    fi
    base64 < signing.p12 | tr -d '\r\n' | gh secret set MACOS_SIGNING_CERT "${target[@]}"
    printf %s "$P12_PASS" | gh secret set MACOS_SIGNING_CERT_PASSWORD "${target[@]}"
    echo "secrets MACOS_SIGNING_CERT and MACOS_SIGNING_CERT_PASSWORD set ($where)"
else
    base64 < signing.p12 | tr -d '\r\n' > signing.p12.base64
    echo "Set the secrets by hand (Settings, Secrets and variables, Actions):"
    echo "  MACOS_SIGNING_CERT           the content of $DIR/signing.p12.base64 (delete the file then)"
    echo "  MACOS_SIGNING_CERT_PASSWORD  the password you just entered"
fi
unset P12_PASS

echo
echo "Keep $DIR/signing.p12 and its password safe (e.g. in a password manager)."
