// 扫码库和它的 wasm 都自托管。默认的 locateFile 会去 CDN 取，这里必须覆盖。
import { prepareZXingModule, readBarcodes } from "zxing-wasm/reader";
import wasmUrl from "zxing-wasm/reader/zxing_reader.wasm?url";

prepareZXingModule({
    overrides: {
        locateFile: (path: string, prefix: string) => (path.endsWith(".wasm") ? wasmUrl : prefix + path),
    },
});

export async function decodeQr(input: ImageData | Blob): Promise<string[]> {
    const results = await readBarcodes(input, { formats: ["QRCode"], tryHarder: true, maxNumberOfSymbols: 1 });
    return results.filter((result) => result.isValid).map((result) => result.text);
}
