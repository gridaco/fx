import grida.fx


def test_the_sdk_imports() -> None:
    assert set(grida.fx.__all__) == {
        "Agent",
        "CallFailed",
        "CallRefused",
        "CallResult",
        "CapabilityError",
        "CeilingExceeded",
        "Ctx",
        "EngineError",
        "FxError",
        "Group",
        "InputFile",
        "NodeFailure",
        "NodeSpec",
        "Output",
        "Plan",
        "PlanRefused",
        "PortSpec",
        "RunResult",
        "SpecError",
        "StepRef",
        "Tool",
        "ToolInvocationError",
        "ToolReply",
        "ToolResult",
        "Workflow",
        "node",
        "param",
        "plan",
        "plan_async",
        "run",
        "run_async",
        "spec_of",
        "tool",
    }


def test_the_host_module_imports() -> None:
    import grida.fx.host

    assert callable(grida.fx.host.main)


def test_the_standard_bodies_import() -> None:
    import grida.fx.std

    assert sorted(grida.fx.std.BODIES) == [
        "files.copy",
        "image.check_alpha",
        "image.check_size",
        "image.crop",
        "image.mirror_repeat",
        "image.pad",
        "image.resize",
        "json.merge",
        "package",
    ]
