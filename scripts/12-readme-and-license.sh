#!/usr/bin/env bash
# 12-readme-and-license.sh - create README.md, LICENSE.md (MIT),
# fix Cargo.toml. Uses base64-encoded payloads to avoid any
# heredoc / backtick quoting issues. Idempotent.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR" >&2; return 1 2>/dev/null || exit 1; }

if [[ ! -f Cargo.toml ]]; then
  echo "ERROR: not at repo root" >&2
  return 1 2>/dev/null || exit 1
fi

echo "====== README + LICENSE START"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

python3 - <<'PYEOF'
import base64
import re
from pathlib import Path

ROOT = Path.cwd()

# All payloads are base64-encoded plain UTF-8 text. They were generated
# from the canonical source by running:
#   python3 -c "import base64; print(base64.b64encode(open('FILE').read().encode()).decode())"
# Keeping them as base64 in the script avoids nested heredoc and
# backtick-fence parsing issues across shells.

README_B64 = (
    "IyBTZW50aW5lbC1sYW5nCgoqKkEgc2VjdXJpdHktZmlyc3Qgc3lzdGVtcyBwcm9ncmFtbWluZyBs"
    "YW5ndWFnZSBmb3IgdGhlIHRocmVhdHMgb2YgdGhlIDIwMzBzLioqCgpTZW50aW5lbCBpcyBhIG1l"
    "bW9yeS1zYWZlLCBjYXBhYmlsaXR5LWJvdW5kZWQgc3lzdGVtcyBsYW5ndWFnZSBiZWluZyBidWls"
    "dCBieSBbQW5pZSBMdGQuXShodHRwczovL2FuaWVzb2x1dGlvbnMuYWkpIHRvIGFkZHJlc3MgdGhl"
    "IGJ1ZyBjbGFzc2VzIHRoYXQgZG9taW5hdGUgbW9kZXJuIHNlY3VyaXR5IGluY2lkZW50cyDigJQg"
    "c3VwcGx5LWNoYWluIGF0dGFja3MsIGNyeXB0b2dyYXBoaWMgc2lkZSBjaGFubmVscywgc2VjcmV0"
    "IGRpc2Nsb3N1cmUsIHVudHJ1c3RlZC1jb2RlIGV4ZWN1dGlvbiwgYW5kIGluZm9ybWF0aW9uLWZs"
    "b3cgdmlvbGF0aW9ucyDigJQgbm9uZSBvZiB3aGljaCBhcmUgc3RydWN0dXJhbGx5IGFkZHJlc3Nl"
    "ZCBieSBhbnkgcHJvZHVjdGlvbiBsYW5ndWFnZSB0b2RheS4KCkZvciB0aGUgc2hvcnQtZm9ybSBw"
    "aXRjaCwgc2VlIFtgZG9jcy9TRU5USU5FTF9TVU1NQVJZLm1kYF0oZG9jcy9TRU5USU5FTF9TVU1N"
    "QVJZLm1kKS4KRm9yIHRoZSBmdWxsIGRlc2lnbiwgc2VlIFtgZG9jcy9TRU5USU5FTF9ERVNJR04u"
    "bWRgXShkb2NzL1NFTlRJTkVMX0RFU0lHTi5tZCkgYW5kCltgZG9jcy9TRU5USU5FTF9ERVNJR04y"
    "Lm1kYF0oZG9jcy9TRU5USU5FTF9ERVNJR04yLm1kKS4KCiMjIFN0YXR1cwoKU2VudGluZWwgaXMg"
    "aW4gYWN0aXZlIGVhcmx5LXN0YWdlIGRldmVsb3BtZW50LiBUaGlzIGlzIGEgbXVsdGkteWVhciBy"
    "ZXNlYXJjaCBwcm9qZWN0OyBub3RoaW5nIGhlcmUgaXMgcHJvZHVjdGlvbi1yZWFkeS4gVGhlIGlt"
    "cGxlbWVudGF0aW9uIGlzIHN0YWdlZCBpbnRvIGZvdXIgcGhhc2VzIChzZWUgW2Bkb2NzL0hBTkRP"
    "VkVSLm1kYF0oZG9jcy9IQU5ET1ZFUi5tZCkgZm9yIHRoZSBmdWxsIHJhdGlvbmFsZSk6CgotIFt4"
    "XSAqKlBoYXNlIEEg4oCUIE1lbW9yeSBCcm9rZXIgcHJvdG90eXBlKiogKFJ1c3QgY3JhdGUsIGNv"
    "bXBsZXRlKQogIC0gW3hdIEEw4oCTQTg6IGdlbmVyYXRpb25hbCBhcmVuYXMsIHR3byBhbGxvY2F0"
    "aW9uIHN0cmF0ZWdpZXMsIHNjb3BlZCBidWRnZXRzLCBzdGF0cyBhbmQgZGlhZ25vc3RpY3MsIHJl"
    "Y29yZGluZyBtb2RlLCBzZWNyZXQtbWVtb3J5IHBvbGljeSAobWxvY2sgKyB6ZXJvLW9uLWZyZWUp"
    "LCB2YWxpZGF0aW9uIGV4YW1wbGUgcHJvZ3JhbXMKICAtIFt4XSBBOTogZmFsbGlibGUgYnVpbGRl"
    "cnMsIHN0cnVjdHVyZWQgT1MtZXJyb3IgZGV0YWlsIGluIGBCcm9rZXJFcnJvcmAKLSBbIF0gKipQ"
    "aGFzZSBCIOKAlCBTZW50aW5lbC1NaW5pIGVmZmVjdHMgcHJvdG90eXBlKiogKHRyZWUtd2Fsa2lu"
    "ZyBpbnRlcnByZXRlciwgaW4gcHJvZ3Jlc3MpCiAgLSBbeF0gQjA6IGxleGVyICsgcmVjdXJzaXZl"
    "LWRlc2NlbnQgcGFyc2VyICsgZXZhbHVhdG9yIGZvciBhIHB1cmUgZXhwcmVzc2lvbiBjYWxjdWx1"
    "cyAobm8gdHlwZXMsIG5vIGVmZmVjdHMgeWV0KQogIC0gWyBdIEIxOiBIaW5kbGV5LU1pbG5lciB0"
    "eXBlIGluZmVyZW5jZSwgYGxldHJlY2AsIHNwYW4tdHJhY2tlZCBkaWFnbm9zdGljcwogIC0gWyBd"
    "IEIyOiBlZmZlY3Qgcm93cyBhbmQgZWZmZWN0IGRlY2xhcmF0aW9ucwogIC0gWyBdIEIzOiBlZmZl"
    "Y3QgaGFuZGxlcnMgKGBoYW5kbGUg4oCmIHdpdGgg4oCmYCkKICAtIFsgXSBCNDogYHNlY3JldCBU"
    "YCBxdWFsaWZpZXIgd2l0aCBjb25zdGFudC10aW1lIGNoZWNrCi0gWyBdICoqUGhhc2UgQyDigJQg"
    "Qm9vdHN0cmFwIGNvbXBpbGVyKiogKHByb2R1Y3Rpb24gUnVzdCBpbXBsZW1lbnRhdGlvbiBvZiBm"
    "dWxsIFNlbnRpbmVsLCB0YXJnZXRzIExMVk0pCi0gWyBdICoqUGhhc2UgRCDigJQgU2VsZi1ob3N0"
    "aW5nKiogKFNlbnRpbmVsIGNvbXBpbGVyIHdyaXR0ZW4gaW4gU2VudGluZWwpCgpDdXJyZW50IHRl"
    "c3QgY292ZXJhZ2U6CgotIGBzZW50aW5lbC1icm9rZXJgOiAgICAgICAgNjkgdGVzdHMgKyAxIGRv"
    "Y3Rlc3QsIGNsaXBweSBjbGVhbiB1bmRlciBgLUQgd2FybmluZ3NgCi0gYHNlbnRpbmVsLWVmZmVj"
    "dHMtcHJvdG9gOiAyMyB0ZXN0cywgY2xpcHB5IGNsZWFuIHVuZGVyIGAtRCB3YXJuaW5nc2AKCkZv"
    "ciB0aGUgYXV0aG9yaXRhdGl2ZSBzdGF0ZSBvZiB0aGUgY29kZWJhc2UsIHNlZQpbYGRvY3MvU1RB"
    "VEUubWRgXShkb2NzL1NUQVRFLm1kKS4gV2hlbiBTVEFURS5tZCBhbmQgYW55IG90aGVyIGRvY3Vt"
    "ZW50CmRpc2FncmVlLCBTVEFURS5tZCB3aW5zLgoKIyMgV2hhdCB3b3JrcyB0b2RheQoKVGhlIGJy"
    "b2tlciBjcmF0ZSBpcyBmZWF0dXJlLWNvbXBsZXRlIGZvciBQaGFzZSBBIGFuZCBydW5uYWJsZS4g"
    "VGhyZWUKZXhhbXBsZSBwcm9ncmFtcyBleGVyY2lzZSB0aGUgZnVsbCBzdXJmYWNlOgoKYGBgYmFz"
    "aApjYXJnbyBydW4gLXAgc2VudGluZWwtYnJva2VyIC0tZXhhbXBsZSB0b2tlbl9idWNrZXQKY2Fy"
    "Z28gcnVuIC1wIHNlbnRpbmVsLWJyb2tlciAtLWV4YW1wbGUgcmVxdWVzdF9waXBlbGluZQpjYXJn"
    "byBydW4gLXAgc2VudGluZWwtYnJva2VyIC0tZXhhbXBsZSBjcmVkZW50aWFsX3N0b3JlCmBgYAoK"
    "VGhlIGBjcmVkZW50aWFsX3N0b3JlYCBkZW1vIGlzIHRoZSBtb3N0IGNvbmNyZXRlIGRlbW9uc3Ry"
    "YXRpb24gb2YKU2VudGluZWwncyBzZWN1cml0eSB0aGVzaXMgYXZhaWxhYmxlIHRvZGF5LiBJdCBh"
    "bGxvY2F0ZXMgY3JlZGVudGlhbHMKaW50byBhIHNsYWIgYXJlbmEgd2l0aCBgbWxvY2tgICsgemVy"
    "by1vbi1mcmVlIHBvbGljeSBhY3RpdmUsIGhleC1kdW1wcwp0aGUgcmF3IG1lbW9yeSBiZWZvcmUg"
    "YW5kIGFmdGVyIGBmcmVlKClgLCBhbmQgdmVyaWZpZXMgdGhhdCB0aGUKNjQtYnl0ZSBzbG90IGlz"
    "IGZ1bGx5IHplcm9lZCB3aGVuIHRoZSBjcmVkZW50aWFsIGlzIHJlbGVhc2VkLgoKIyMgQnVpbGQK"
    "ClJlcXVpcmVtZW50czogUnVzdCBzdGFibGUgKDEuODArKSwgYGNhcmdvLW5leHRlc3RgIHJlY29t"
    "bWVuZGVkLgoKYGBgYmFzaApnaXQgY2xvbmUgaHR0cHM6Ly9naXRodWIuY29tL2FyY2FuaWkvU2Vu"
    "dGluZWwtbGFuZy5naXQKY2QgU2VudGluZWwtbGFuZwpjYXJnbyBidWlsZCAtLXdvcmtzcGFjZQpj"
    "YXJnbyBuZXh0ZXN0IHJ1biAtLXdvcmtzcGFjZSAgICAgICAgIyBvciBgY2FyZ28gdGVzdCAtLXdv"
    "cmtzcGFjZWAKY2FyZ28gY2xpcHB5IC0td29ya3NwYWNlIC0tYWxsLXRhcmdldHMgLS0gLUQgd2Fy"
    "bmluZ3MKYGBgCgojIyBSZXBvc2l0b3J5IGxheW91dAoKLSBgY3JhdGVzL3NlbnRpbmVsLWJyb2tl"
    "ci9gIOKAlCBQaGFzZSBBIGRlbGl2ZXJhYmxlLCBjb21wbGV0ZS4KLSBgY3JhdGVzL3NlbnRpbmVs"
    "LWVmZmVjdHMtcHJvdG8vYCDigJQgUGhhc2UgQjogU2VudGluZWwtTWluaSBpbnRlcnByZXRlci4K"
    "LSBgY3JhdGVzL3NlbnRpbmVsLXtzeW50YXgsYXN0LHJlc29sdmUsdHlwZXMsaGlyLG1pcixjb2Rl"
    "Z2VuLGRyaXZlcixydW50aW1lLGxzcH0vYCDigJQgUGhhc2UgQyBzY2FmZm9sZHMgKHN0dWIgY3Jh"
    "dGVzKS4KLSBgZG9jcy9gIOKAlCBkZXNpZ24sIHN0YXR1cywgYW5kIHByb2Nlc3MgZG9jdW1lbnRz"
    "OgogIC0gW2BTRU5USU5FTF9TVU1NQVJZLm1kYF0oZG9jcy9TRU5USU5FTF9TVU1NQVJZLm1kKSDi"
    "gJQgb25lLXBhZ2UgcGl0Y2gKICAtIFtgU0VOVElORUxfREVTSUdOLm1kYF0oZG9jcy9TRU5USU5F"
    "TF9ERVNJR04ubWQpIGFuZCBbYFNFTlRJTkVMX0RFU0lHTjIubWRgXShkb2NzL1NFTlRJTkVMX0RF"
    "U0lHTjIubWQpIOKAlCBmdWxsIGRlc2lnbgogIC0gW2BIQU5ET1ZFUi5tZGBdKGRvY3MvSEFORE9W"
    "RVIubWQpIOKAlCBpbXBsZW1lbnRhdGlvbiBwbGFuIGFuZCB3b3JraW5nIG5vcm1zCiAgLSBbYFNU"
    "QVRFLm1kYF0oZG9jcy9TVEFURS5tZCkg4oCUIGN1cnJlbnQgaW1wbGVtZW50YXRpb24gc3RhdGUg"
    "KHNvdXJjZSBvZiB0cnV0aCkKICAtIFtgQkFDS0xPRy5tZGBdKGRvY3MvQkFDS0xPRy5tZCkg4oCU"
    "IHBvc3QtMS4wIGJhY2tsb2cgYW5kIHJlc2VhcmNoIGRpcmVjdGlvbnMKICAtIFtgU0VDUkVUU19M"
    "SUZFQ1lDTEUubWRgXShkb2NzL1NFQ1JFVFNfTElGRUNZQ0xFLm1kKSDigJQgc2VjcmV0LW1lbW9y"
    "eSBkZXNpZ24KICAtIFtgVElFUkVEX1JFTEVBU0VTLm1kYF0oZG9jcy9USUVSRURfUkVMRUFTRVMu"
    "bWQpIOKAlCByZWxlYXNlIHRpZXJzCiAgLSBbYGRlY2lzaW9ucy9gXShkb2NzL2RlY2lzaW9ucy8p"
    "IOKAlCBhcmNoaXRlY3R1cmUgZGVjaXNpb24gcmVjb3JkcyAoMSBzbyBmYXIpCi0gYHNjcmlwdHMv"
    "YCDigJQgcGF0Y2ggc2NyaXB0cyB0aGF0IGJ1aWx0IGVhY2ggbWlsZXN0b25lLCBuYW1lZCBgTk4t"
    "PHBoYXNlPi5zaGAuCgojIyBXaG8ncyBidWlsZGluZyB0aGlzCgpTZW50aW5lbCBpcyBiZWluZyBi"
    "dWlsdCBieSBbQW5pZSBMdGQuXShodHRwczovL2FuaWVzb2x1dGlvbnMuYWkpIGFzIHRoZSBsYW5n"
    "dWFnZSBzdWJzdHJhdGUgZm9yIHNlY3VyaXR5LWNyaXRpY2FsIHByb2R1Y3RzIHRhcmdldGluZyBi"
    "YW5rcywgZ292ZXJubWVudHMsIGFuZCByZWd1bGF0ZWQgaW5kdXN0cmllcy4gU2VudGluZWwgaXMg"
    "b3Blbi1zb3VyY2U7IHRoZSBwcm9kdWN0cyBidWlsdCBvbiB0b3Agb2YgaXQgYXJlIEFuaWUncyBj"
    "b21tZXJjaWFsIHdvcmsuCgojIyBXaGF0IHRoaXMgaXMgbm90CgotICoqTm90IHByb2R1Y3Rpb24t"
    "cmVhZHkuKiogVGhlIGJyb2tlciBpcyBhIHdvcmtpbmcgUnVzdCBjcmF0ZTsKICBTZW50aW5lbC10"
    "aGUtbGFuZ3VhZ2UgZG9lcyBub3QgeWV0IGNvbXBpbGUgYW55IHJlYWwgcHJvZ3JhbXMuCi0gKipO"
    "b3Qgc3RhYmxlLioqIEV2ZXJ5IEFQSSBpbiB0aGlzIHJlcG9zaXRvcnkgY2FuIGNoYW5nZSBhdCBh"
    "bnkgdGltZS4KICBObyBzZW12ZXIgZ3VhcmFudGVlcywgbm8gcHVibGljIHJlbGVhc2UuCi0gKipO"
    "b3QgYWNjZXB0aW5nIGdlbmVyYWwgY29udHJpYnV0aW9ucyB5ZXQuKiogVGhlIGRlc2lnbiBpcyBz"
    "dGlsbAogIGZsdWlkOyBhIGNvbnRyaWJ1dG9yIG9uYm9hcmRpbmcgcHJvY2VzcyB3aWxsIGNvbWUg"
    "b25jZSB0aGUgY29yZQogIHNoYXBlIHN0YWJpbGlzZXMgYWZ0ZXIgUGhhc2UgQi4KLSAqKk5vdCBt"
    "YWtpbmcgc2VjdXJpdHkgY2xhaW1zIHRvZGF5LioqIFNlbnRpbmVsIHdpbGwgZXZlbnR1YWxseQog"
    "IGVuZm9yY2Ugc3Ryb25nIHNlY3VyaXR5IHByb3BlcnRpZXMgYXQgdGhlIGxhbmd1YWdlIGxheWVy"
    "LiBOb25lIG9mCiAgdGhvc2UgcHJvcGVydGllcyBleGlzdCB5ZXQgZm9yIGVuZC11c2VyIGNvZGU7"
    "IG9ubHkgdGhlIGJyb2tlcidzCiAgaW50ZXJuYWwgaW52YXJpYW50cyBhcmUgdGVzdGVkIGFuZCBl"
    "bmZvcmNlZC4KCiMjIExpY2Vuc2UKCk1JVCDigJQgc2VlIFtgTElDRU5TRS5tZGBdKExJQ0VOU0Uu"
    "bWQpLgo="
)

LICENSE_B64 = (
    "TUlUIExpY2Vuc2UKCkNvcHlyaWdodCAoYykgMjAyNiBBbmllIEx0ZC4KClBlcm1pc3Npb24gaXMg"
    "aGVyZWJ5IGdyYW50ZWQsIGZyZWUgb2YgY2hhcmdlLCB0byBhbnkgcGVyc29uIG9idGFpbmluZwphIGNv"
    "cHkgb2YgdGhpcyBzb2Z0d2FyZSBhbmQgYXNzb2NpYXRlZCBkb2N1bWVudGF0aW9uIGZpbGVzICh0aGUK"
    "IlNvZnR3YXJlIiksIHRvIGRlYWwgaW4gdGhlIFNvZnR3YXJlIHdpdGhvdXQgcmVzdHJpY3Rpb24sIGlu"
    "Y2x1ZGluZwp3aXRob3V0IGxpbWl0YXRpb24gdGhlIHJpZ2h0cyB0byB1c2UsIGNvcHksIG1vZGlmeSwg"
    "bWVyZ2UsIHB1Ymxpc2gsCmRpc3RyaWJ1dGUsIHN1YmxpY2Vuc2UsIGFuZC9vciBzZWxsIGNvcGllcyBv"
    "ZiB0aGUgU29mdHdhcmUsIGFuZCB0bwpwZXJtaXQgcGVyc29ucyB0byB3aG9tIHRoZSBTb2Z0d2FyZSBp"
    "cyBmdXJuaXNoZWQgdG8gZG8gc28sIHN1YmplY3QgdG8KdGhlIGZvbGxvd2luZyBjb25kaXRpb25zOgoK"
    "VGhlIGFib3ZlIGNvcHlyaWdodCBub3RpY2UgYW5kIHRoaXMgcGVybWlzc2lvbiBub3RpY2Ugc2hhbGwg"
    "YmUKaW5jbHVkZWQgaW4gYWxsIGNvcGllcyBvciBzdWJzdGFudGlhbCBwb3J0aW9ucyBvZiB0aGUgU29m"
    "dHdhcmUuCgpUSEUgU09GVFdBUkUgSVMgUFJPVklERUQgIkFTIElTIiwgV0lUSE9VVCBXQVJSQU5UWSBP"
    "RiBBTlkgS0lORCwKRVhQUkVTUyBPUiBJTVBMSUVELCBJTkNMVURJTkcgQlVUIE5PVCBMSU1JVEVEIFRP"
    "IFRIRSBXQVJSQU5USUVTIE9GCk1FUkNIQU5UQUJJTElUWSwgRklUTkVTUyBGT1IgQSBQQVJUSUNVTEFS"
    "IFBVUlBPU0UgQU5EIE5PTklORlJJTkdFTUVOVC4KSU4gTk8gRVZFTlQgU0hBTEwgVEhFIEFVVEhPUlMg"
    "T1IgQ09QWVJJR0hUIEhPTERFUlMgQkUgTElBQkxFIEZPUiBBTlkKQ0xBSU0sIERBTUFHRVMgT1IgT1RI"
    "RVIgTElBQklMSVRZLCBXSEVUSEVSIElOIEFOIEFDVElPTiBPRiBDT05UUkFDVCwKVE9SVCBPUiBPVEhF"
    "UldJU0UsIEFSSVNJTkcgRlJPTSwgT1VUIE9GIE9SIElOIENPTk5FQ1RJT04gV0lUSCBUSEUKU09GVFdB"
    "UkUgT1IgVEhFIFVTRSBPUiBPVEhFUiBERUFMSU5HUyBJTiBUSEUgU09GVFdBUkUuCg=="
)

def write_decoded(path: Path, b64_payload: str):
    content = base64.b64decode(b64_payload).decode("utf-8")
    if path.exists() and path.read_text() == content:
        print(f"  UNCHANGED {path.relative_to(ROOT)}")
        return
    action = "UPDATE" if path.exists() else "CREATE"
    path.write_text(content)
    print(f"  {action} {path.relative_to(ROOT)}")

write_decoded(ROOT / "README.md", README_B64)
write_decoded(ROOT / "LICENSE.md", LICENSE_B64)

# ---- Cargo.toml fixes ------------------------------------------------------
cargo = ROOT / "Cargo.toml"
txt = cargo.read_text()
orig = txt

# 1. Fix repository URL.
txt = txt.replace(
    'repository   = "https://github.com/bryan/Sentinel-language"',
    'repository   = "https://github.com/arcanii/Sentinel-lang"',
)

# 2. There is no `license = ...` at the workspace level today (member
#    crates set their own via license.workspace = true reading the
#    workspace.package.license field — which is also not present). Add
#    `license = "MIT"` to [workspace.package] if missing.
if 'license      = ' not in txt and 'license =' not in txt.split('[workspace.dependencies]')[0]:
    txt = re.sub(
        r'(repository   = "https://github\.com/arcanii/Sentinel-lang")',
        r'\1\nlicense      = "MIT"',
        txt,
        count=1,
    )

if txt != orig:
    cargo.write_text(txt)
    print("  UPDATE Cargo.toml (repository URL + license = MIT)")
else:
    print("  UNCHANGED Cargo.toml")
PYEOF
PATCH_RC=$?

echo
echo "====== PATCH DONE (rc=$PATCH_RC)"
echo
echo "====== HEAD OF README.md"
head -20 README.md
echo
echo "====== HEAD OF LICENSE.md"
head -5 LICENSE.md
echo
echo "====== Cargo.toml workspace.package"
sed -n '/\[workspace.package\]/,/^\[/p' Cargo.toml | sed '$d'
echo
echo "====== SANITY: workspace builds"
cargo build --workspace 2>&1 | tail -6
echo
echo "====== README + LICENSE END"
