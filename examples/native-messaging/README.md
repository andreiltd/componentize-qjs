## componentize-qjs Native Messaging host

### Install dependencies

```shell
bun install --cache-dir ./.bun-cache componentize-qjs
```

### WASI P2

#### WIT
```shell
cargo bininstall wkg
wkg wit fetch -d . --cache ./.wit-cache
```

#### Compile
```shell
bun x componentize-qjs -n native-messaging-componentize-qjs -w . -m \
--opt-size --js nm_componentize_qjs.js -o nm_componentize_qjs.wasm
```

#### Run
```bash
#!/usr/bin/env -S wasmtime run -W component-model-async=y /ABSOLUTE/PATH/TO/native-messaging-componentize-qjs/nm_componentize_qjs.wasm 
```
### WASI P3

#### WIT
```shell
wkg wit fetch -d ./p3 --cache ./.wit-cache
```

#### Compile

```shell
bun x componentize-qjs -n native-messaging-componentize-qjs-p3 -w ./p3 -m \
--opt-size --js ./p3/nm_componentize_qjs_p3.js -o ./p3/nm_componentize_qjs_p3.wasm
```

#### Run

```bash
#!/usr/bin/env -S wasmtime run -W component-model-async=y -S p3=y /ABSOLUTE/PATH/TO/native-messaging-componentize-qjs/p3/nm_componentize_qjs_p3.wasm
```
# Installation and usage on Chrome and Chromium

1. Navigate to `chrome://extensions`.
2. Toggle `Developer mode`.
3. Click `Load unpacked`.
4. Select `native-messaging-componentize-qjs` folder.
5. Note the generated extension ID.
6. Open `nm_componentize_qjs.json` in a text editor, set `"path"` to absolute path of `nm_componentize_qjs.sh` and `chrome-extension://<ID>/` using ID from 5 in `"allowed_origins"` array, and set `nm_componentize_qjs.sh` permission to executable.
7. Copy the file to Chrome or Chromium configuration folder, e.g., Chromium on Linux `~/.config/chromium/NativeMessagingHosts`; Chrome dev channel on Linux `~/.config/google-chrome-unstable/NativeMessagingHosts`.
8. To test click `service worker` link in panel of unpacked extension which is DevTools for `background.js` in MV3 `ServiceWorker`, observe echo'ed message from componentize-qjs Native Messaging host. To disconnect run `port.disconnect()`.

The Native Messaging host echoes back the message passed. 

For differences between OS and browser implementations see [Chrome incompatibilities](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Chrome_incompatibilities#native_messaging).

# Testing

## Deno 
[nm_standalone_test.js](https://github.com/guest271314/NativeMessagingHosts/blob/main/nm_standalone_test.js) 

```shell
nm_standalone_test.js ./nm_componentize_qjs.sh
```
## Node.js 
[nm_standalone_test_node.js](https://github.com/guest271314/NativeMessagingHosts/blob/main/nm_standalone_test_node.js)
```shell
nm_standalone_test_node.js ./nm_componentize_qjs.sh
```

## License
Do What the Fuck You Want to Public License [WTFPLv2](http://www.wtfpl.net/about/)


