# Source Version Information

This file documents the exact version of the original source code that our patches are based on.

## Original Source
- **URL**: https://gist.github.com/steipete/8396e512171d31e934f0013e5651691e
- **Raw URL**: https://gist.githubusercontent.com/steipete/8396e512171d31e934f0013e5651691e/raw/214162cb78163db044c522e3c1cc630e6753edb3/statusline.rs
- **SHA256 Hash**: `5f7851061abbd896c2d4956323fa85848df79242448019bbea7799111d3cebda`

## Hash Validation

The build process automatically validates the downloaded file against this hash to ensure:
1. We're patching the correct version of the original code
2. The patch will apply cleanly
3. No unexpected changes have been made to the original

## If Hash Validation Fails

If you see a hash mismatch error during build:

1. **Check if the original gist was updated**: Compare the new content with our patches
2. **Update the patch file if needed**: 
   ```bash
   # Download new version
   curl -s [gist_url] -o statusline.rs.new
   
   # Review changes
   diff statusline.rs.orig statusline.rs.new
   
   # Update patch if changes are compatible
   diff -u statusline.rs.new statusline-modified.rs > statusline.patch
   
   # Update the hash in Makefile
   sha256sum statusline.rs.new
   ```
3. **Update EXPECTED_HASH in Makefile** with the new hash
4. **Test thoroughly** to ensure patches still work correctly

## Version History

- **2024-11-22**: Initial version based on gist revision 214162cb78163db044c522e3c1cc630e6753edb3