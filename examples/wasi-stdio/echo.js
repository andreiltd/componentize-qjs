import stdin from "wasi:cli/stdin@0.3.0-rc-2026-03-15";
import stdout from "wasi:cli/stdout@0.3.0-rc-2026-03-15";

export const run = {
    async run() {
        const [input, status] = stdin.readViaStream();
        const written = await stdout.writeViaStream(input);
        if (written.tag === "err") {
            throw written.val;
        }
        const statusResult = await status.read();
        if (statusResult.tag === "err") {
            throw statusResult.val;
        }
    },
};
