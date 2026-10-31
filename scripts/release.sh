#!/usr/bin/env bash
# 给 CI 建好的草稿 Release 签名,然后转为正式发布.在仓库根目录运行.
#
#   scripts/release.sh v1.2.3 --key <key.der.gpg> --cert <cert.pem>
#
# 口令由 gpg 询问,脚本不读取.私钥只经过管道,不写文件.
set -euo pipefail

# CA 证书和 APK 证书共用一把私钥,传错证书时 apksigner 不会报错,所以固定指纹.
APK_CERT_SHA256=d9f761bbbaf9cf637e579a88c69b04391de49d1853f6a078424f098edbe97c73
PACKAGE=com.ccobcode.bitone
PUBLIC_KEY=crates/bitone-core/assets/manifest-key.der
MANIFEST=crates/bitone-core/assets/manifest.json

die() {
    echo "发布中止:$*" >&2
    exit 1
}

decrypt() {
    gpg --decrypt --no-symkey-cache --quiet "$key"
}

manifest_version() {
    git show "$1:$MANIFEST" | jq .version
}

tag=${1:-}
[[ $tag =~ ^v[0-9]{1,2}\.[0-9]{1,2}\.[0-9]{1,2}$ ]] || die "tag 要写成 vX.Y.Z,每段不超过 99"
shift
key='' cert=''
while (($# > 1)); do
    case $1 in
    --key) key=$2 ;;
    --cert) cert=$2 ;;
    *) die "未知参数 $1" ;;
    esac
    shift 2
done
(($# == 0)) || die "未知参数 $1"
[[ -f $key && -f $cert ]] || die "要用 --key 和 --cert 指定存在的文件"
[[ -f $MANIFEST ]] || die "要在仓库根目录运行"

android=${ANDROID_HOME:-$HOME/Library/Android/sdk}
build_tools=$(find "$android/build-tools" -mindepth 1 -maxdepth 1 | sort -V | tail -1)
export JAVA_HOME=${JAVA_HOME:-$(/usr/libexec/java_home -v 21)}
export PATH=$build_tools:$JAVA_HOME/bin:$PATH
GPG_TTY=$(tty || true)
export GPG_TTY

# 先做不需要口令的检查.
git fetch --quiet --tags origin main
git merge-base --is-ancestor "$tag" origin/main || die "$tag 不在 main 上"

fingerprint=$(openssl x509 -in "$cert" -noout -fingerprint -sha256 | cut -d= -f2 | tr -d : | tr A-F a-f)
[[ $fingerprint == "$APK_CERT_SHA256" ]] || die "证书指纹是 ${fingerprint},不是 APK 证书"
cmp -s <(openssl x509 -in "$cert" -noout -pubkey | openssl pkey -pubin -outform DER) \
    <(git show "$tag:$PUBLIC_KEY") || die "证书的公钥和 $tag 里的 manifest-key.der 不一致"

[[ $(gh release view "$tag" --json isDraft --jq .isDraft) == true ]] || die "$tag 不是草稿 Release"

dir=dist/$tag
rm -rf "$dir"
mkdir -p "$dir"
gh release download "$tag" --dir "$dir"

unsigned=$dir/bitone-$tag-android-unsigned.apk
signed=$dir/bitone-$tag-android.apk
badging=$(aapt2 dump badging "$unsigned")
[[ $badging == *"package: name='$PACKAGE'"* ]] || die "APK 的包名不是 $PACKAGE"
[[ $badging == *"versionName='${tag#v}'"* ]] || die "APK 的版本号不是 ${tag#v}"
permissions=$(aapt2 dump permissions "$unsigned")
[[ $permissions != *android.permission.INTERNET* ]] || die "APK 申请了 INTERNET 权限"

apksigner sign --key <(decrypt) --cert "$cert" --min-sdk-version 31 --v1-signing-enabled false \
    --v4-signing-enabled false --alignment-preserved true --out "$signed" "$unsigned"
certificates=$(apksigner verify --min-sdk-version 31 --print-certs "$signed")
[[ $certificates == *"SHA-256 digest: $APK_CERT_SHA256"* ]] || die "签名后的证书指纹不对"
assets=("$signed")

# 内置清单的版本比上一个 tag 高时,才发布扫码更新用的清单包.
previous=$(git describe --tags --abbrev=0 "$tag^" 2>/dev/null || true)
if [[ -n $previous ]] && (($(manifest_version "$tag") > $(manifest_version "$previous"))); then
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    git show "$tag:$MANIFEST" >"$work/manifest.json"
    git show "$tag:$PUBLIC_KEY" >"$work/public.der"
    openssl dgst -sha256 -sign <(decrypt) -keyform DER -out "$work/signature" "$work/manifest.json"
    openssl dgst -sha256 -verify "$work/public.der" -keyform DER -signature "$work/signature" \
        "$work/manifest.json" >/dev/null
    package=$dir/bitone-manifest-v$(manifest_version "$tag").bin
    cat "$work/signature" "$work/manifest.json" >"$package"
    assets+=("$package")
fi

(cd "$dir" && shasum -a 256 -- * >SHA256SUMS)
gh release upload "$tag" "${assets[@]}" "$dir/SHA256SUMS" --clobber
gh release edit "$tag" --draft=false
echo "已发布 $tag"
