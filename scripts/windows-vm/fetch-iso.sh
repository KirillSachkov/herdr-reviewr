#!/bin/sh
# Download the Windows 11 ARM64 ISO from Microsoft: fetch-iso.sh <out.iso>
# Replays the request sequence of Fido (https://github.com/pbatard/Fido): whitelist a session,
# pass the ov-df check, list the ARM64 edition's SKUs, then ask for the English SKU's link.
# Microsoft throttles this per IP after a few calls ("Sentinel marked this request as
# rejected"); a link stays valid for 24 hours, and `curl -C -` resumes a cut download.
set -eu
out="$1"
ua="Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0"
sid="$(uuidgen | tr 'A-Z' 'a-z')"
inst="560dc9f3-1aa5-4a2f-b63c-9e18f8d0e175"
prof="606624d44113"
edition=3816 # Windows 11 Home/Pro/Edu, ARM64
c() { curl -sS --fail -A "$ua" "$@"; }
c -o /dev/null "https://vlscppe.microsoft.com/tags?org_id=y6jn8c31&session_id=$sid"
mdt="$(c "https://ov-df.microsoft.com/mdt.js?instanceId=$inst&PageId=si&session_id=$sid")"
w="$(printf '%s' "$mdt" | grep -Eo '[?&]w=[A-F0-9]+' | head -1 | cut -d= -f2)"
rticks="$(printf '%s' "$mdt" | grep -Eo 'rticks="\+?[0-9]+' | head -1 | tr -cd '0-9')"
c -o /dev/null "https://ov-df.microsoft.com/?session_id=$sid&CustomerId=$inst&PageId=si&w=$w&mdt=$(($(date +%s) * 1000))&rticks=$rticks"
skus="$(c "https://www.microsoft.com/software-download-connector/api/getskuinformationbyproductedition?profile=$prof&productEditionId=$edition&SKU=undefined&friendlyFileName=undefined&Locale=en-US&sessionID=$sid")"
sku="$(printf '%s' "$skus" | python3 -c 'import json,sys; print(next(s["Id"] for s in json.load(sys.stdin)["Skus"] if s["Language"] == "English"))')"
links="$(c -H "Referer: https://www.microsoft.com/software-download/windows11" "https://www.microsoft.com/software-download-connector/api/GetProductDownloadLinksBySku?profile=$prof&productEditionId=undefined&SKU=$sku&friendlyFileName=undefined&Locale=en-US&sessionID=$sid")"
url="$(printf '%s' "$links" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["ProductDownloadOptions"][0]["Uri"]) if d.get("ProductDownloadOptions") else sys.exit(f"no link: {d}")')"
curl -fL -C - -o "$out" "$url"
