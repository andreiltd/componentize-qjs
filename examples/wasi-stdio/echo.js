import stdin from "wasi:cli/stdin@0.3.0-rc-2026-01-06";
import stdout from "wasi:cli/stdout@0.3.0-rc-2026-01-06";

export const run = {
    async run() {
        const [input, status] = stdin.readViaStream();
        const written = await stdout.writeViaStream(input);
        if (written.tag === "err") {
            return written;
        }
        return await status.read();
    },
};
