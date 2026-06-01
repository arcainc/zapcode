"""Unit tests for the schema-aware argument validation and prompt generation.

These exercise the pure-Python helpers and do not require the compiled
`zapcode` extension. Run with: python3 tests/test_validation.py
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from zapcode_ai import (  # noqa: E402
    ParamDef,
    ToolDefinition,
    _build_named_args,
    _build_system_prompt,
    _validate_tool_definitions,
)


def expect_error(fn, needle):
    try:
        fn()
    except Exception as err:  # noqa: BLE001
        assert needle in str(err), f"expected '{needle}' in: {err}"
        return
    raise AssertionError(f"expected an error containing '{needle}'")


escalate = ToolDefinition(
    description="Escalate to an assignee.",
    parameters={
        "assignee": ParamDef(type="string"),
        "dueAtMs": ParamDef(type="number"),
        "reason": ParamDef(type="string"),
        "metadata": ParamDef(type="object", optional=True),
    },
    execute=lambda args: args,
)

parse_date = ToolDefinition(
    description="Parse a date.",
    parameters={"date": ParamDef(type="string")},
    execute=lambda args: args,
)

save_payload = ToolDefinition(
    description="Save a payload object.",
    parameters={"payload": ParamDef(type="object")},
    execute=lambda args: args,
)


def test_multi_param_requires_named_object():
    out = _build_named_args(
        "escalateTo",
        escalate,
        [{"assignee": "X", "dueAtMs": 1, "reason": "r"}],
    )
    assert out == {"assignee": "X", "dueAtMs": 1, "reason": "r"}

    expect_error(
        lambda: _build_named_args("escalateTo", escalate, ["X", 1, "r"]),
        "expected one named object argument",
    )


def test_single_param_positional_and_named():
    assert _build_named_args("parseDateMs", parse_date, ["2026-06-03"]) == {"date": "2026-06-03"}
    assert _build_named_args("parseDateMs", parse_date, [{"date": "2026-06-03"}]) == {"date": "2026-06-03"}


def test_single_object_param_keeps_payload_shape():
    # An arbitrary object is the payload itself, not a named wrapper.
    assert _build_named_args("savePayload", save_payload, [{"invoiceId": "inv_1"}]) == {
        "payload": {"invoiceId": "inv_1"}
    }
    # A {payload: {...}} wrapper is treated as the named argument.
    assert _build_named_args("savePayload", save_payload, [{"payload": {"a": 1}}]) == {
        "payload": {"a": 1}
    }


def test_validation_errors():
    expect_error(
        lambda: _build_named_args("escalateTo", escalate, [{"assignee": "X", "dueAtMs": 1}]),
        "missing required parameter 'reason'",
    )
    expect_error(
        lambda: _build_named_args(
            "escalateTo", escalate, [{"assignee": "X", "dueAtMs": "soon", "reason": "r"}]
        ),
        "expected number, got string",
    )
    expect_error(
        lambda: _build_named_args(
            "escalateTo", escalate, [{"assignee": "X", "dueAtMs": 1, "reason": "r", "typo": 1}]
        ),
        "unexpected parameter 'typo'",
    )
    # Arrays are not objects.
    expect_error(
        lambda: _build_named_args(
            "escalateTo", escalate, [{"assignee": "X", "dueAtMs": 1, "reason": "r", "metadata": []}]
        ),
        "expected object, got array",
    )
    # Non-finite numbers are rejected.
    expect_error(
        lambda: _build_named_args(
            "escalateTo", escalate, [{"assignee": "X", "dueAtMs": float("inf"), "reason": "r"}]
        ),
        "expected number, got number",
    )
    # Booleans are not numbers.
    expect_error(
        lambda: _build_named_args(
            "escalateTo", escalate, [{"assignee": "X", "dueAtMs": True, "reason": "r"}]
        ),
        "expected number, got boolean",
    )


def test_optional_none_is_omitted():
    out = _build_named_args(
        "escalateTo",
        escalate,
        [{"assignee": "X", "dueAtMs": 1, "reason": "r", "metadata": None}],
    )
    assert "metadata" not in out


def test_reserved_and_invalid_tool_names():
    expect_error(lambda: _validate_tool_definitions({"console": escalate}), "reserved")
    expect_error(lambda: _validate_tool_definitions({"bad-name": escalate}), "valid JavaScript identifiers")


def test_prompt_has_declarations_and_named_call_shape():
    prompt = _build_system_prompt({"escalateTo": escalate, "parseDateMs": parse_date})
    assert "declare function escalateTo(input: {" in prompt
    assert "declare function parseDateMs(date: string): Promise<unknown>;" in prompt
    assert "await escalateTo({ assignee: string" in prompt
    assert "await parseDateMs(date: string)" in prompt


def main():
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    for t in tests:
        t()
        print(f"  ✓ {t.__name__}")
    print(f"\n{len(tests)} python validation checks passed.")


if __name__ == "__main__":
    main()
