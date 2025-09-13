# Roblox PackageLink Place Updater

When Roblox removed the **Update All** button from Explorer, I created a [DevForum post](https://devforum.roblox.com/t/missing-update-all-option-for-package/3679796) to raise the issue. Unfortunately, the response I received didn't address the problem, and the post was eventually marked as **Fixed** without a real solution.

This project was built to fix that - and because I was kind of bored.

---
## What it does

This scans every place in your experience/universe for PackageLinks. Then it fetches the latest PackageLink asset and replaces the old version with the new one. After that, it publishes your places, with your permission first. Before publishing, it also generates local files so you can review the changes if you like.

---

## Requirements

To use this tool, you'll need:

1. An **API Key** with the following permissions:

   - `universe:write`
   - `universe-place:write`

2. Your **.ROBLOSECURITY** cookie.

   - Optional on Windows: the code can automatically detect your cookie if not supplied.

3. Your **Universe ID**.

---

## Disclaimer

Use this project **at your own risk**. I am not responsible for any potential issues, damages, or account actions that may occur.

---

## References & Resources

- [Rojo](https://github.com/rojo-rbx/rojo/blob/master/src/cli/build.rs#L172)
- [Roblox Open Cloud](https://create.roblox.com/docs/cloud)
- [DevForum post on mass-updating places](https://devforum.roblox.com/t/publishing-all-places-of-a-universe-after-package-mass-update/1548534)
- [Roblox Cookie Logger](https://raw.githubusercontent.com/SertraFurr/Roblox-Client-Cookie-Stealer/refs/heads/main/main.py)



