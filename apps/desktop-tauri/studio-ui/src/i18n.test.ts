import { describe, expect, it } from "vitest";

import { messages, translate, type MsgKey } from "./i18n";

describe("i18n", () => {
  it("translates a key into both languages", () => {
    expect(translate("en", "btn.run")).toBe("Run");
    expect(translate("zh", "btn.run")).toBe("运行");
  });

  it("provides en and zh for every key", () => {
    for (const [key, value] of Object.entries(messages)) {
      expect(value.en, `${key}.en`).toBeTruthy();
      expect(value.zh, `${key}.zh`).toBeTruthy();
    }
  });

  it("falls back to the key when missing", () => {
    expect(translate("en", "does.not.exist" as MsgKey)).toBe("does.not.exist");
  });
});
