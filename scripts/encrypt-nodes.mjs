import { readFileSync, writeFileSync } from "node:fs";
import { createCipheriv, randomBytes } from "node:crypto";

const key = Buffer.from(
  "2dbf43f277a10979cb9e06e72b0ddb710cb60d23b1b7c4654aa1f673467bd969",
  "hex",
);
const [inputPath, outputPath] = process.argv.slice(2);

if (!inputPath || !outputPath) {
  console.error("用法: node scripts/encrypt-nodes.mjs <明文节点.json> <输出.json>");
  process.exit(1);
}

const nodes = JSON.parse(readFileSync(inputPath, "utf8"));
if (!Array.isArray(nodes) || !nodes.every((node) => typeof node === "string")) {
  throw new Error("明文节点文件必须是 URL 字符串数组");
}

const nonce = randomBytes(12);
const cipher = createCipheriv("aes-256-gcm", key, nonce);
const ciphertext = Buffer.concat([
  cipher.update(JSON.stringify(nodes), "utf8"),
  cipher.final(),
  cipher.getAuthTag(),
]);
const catalog = {
  version: 1,
  nonce: nonce.toString("base64"),
  ciphertext: ciphertext.toString("base64"),
};

writeFileSync(outputPath, `${JSON.stringify(catalog, null, 2)}\n`);
