import grida.fx


def test_the_sdk_imports() -> None:
    assert set(grida.fx.__all__) == {
        "DECLINE",
        "Agent",
        "Answer",
        "CallFailed",
        "CallRefused",
        "CallResult",
        "CapabilityError",
        "CeilingExceeded",
        "Ctx",
        "EngineError",
        "Failure",
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
        "RunRecord",
        "RunObservationError",
        "RunControlError",
        "RunControlResult",
        "SpecError",
        "StandInCall",
        "StepRef",
        "Tool",
        "ToolInvocationError",
        "ToolReply",
        "ToolResult",
        "Workflow",
        "cancel",
        "cancel_async",
        "inspect_control",
        "inspect_control_async",
        "load_run",
        "load_run_async",
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
