// @vitest-environment jsdom
import { render } from "@testing-library/react";
import { afterEach } from "vitest";
import { describe, expect, it } from "vitest";

import { ParamField } from "./ParamField";
import type { ParamSpec } from "../graph/nodeSpecs";

const engineSpec: ParamSpec = {
  key: "engine",
  label: "Engine",
  control: "select",
  options: ["rules", "onnx_defect"],
  defaultValue: "rules",
};

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ParamField select option states", () => {
  it("disables options the probe reports as unavailable", () => {
    const { container } = render(
      <ParamField
        spec={engineSpec}
        value="rules"
        onChange={() => {}}
        optionStates={{
          rules: { available: true, reason: "built-in CPU rule layer" },
          onnx_defect: { available: false, reason: "missing optional dependency: onnxruntime" },
        }}
      />,
    );
    const options = Array.from(container.querySelectorAll("option"));
    const byValue = Object.fromEntries(options.map((o) => [o.value, o]));
    expect(byValue.rules.disabled).toBe(false);
    expect(byValue.onnx_defect.disabled).toBe(true);
    expect(byValue.onnx_defect.title).toContain("onnxruntime");
  });

  it("leaves every option enabled when no probe states are provided", () => {
    const { container } = render(
      <ParamField spec={engineSpec} value="rules" onChange={() => {}} />,
    );
    const options = Array.from(container.querySelectorAll("option"));
    expect(options.every((o) => !o.disabled)).toBe(true);
  });
});
