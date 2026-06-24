import stdin from "wasi:cli/stdin@0.3.0";
import stdout from "wasi:cli/stdout@0.3.0";

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
