import grida.fx


def test_the_sdk_imports() -> None:
    assert set(grida.fx.__all__) == {
        "DECLINE",
        "Accepted",
        "Adapter",
        "AdapterCall",
        "AdapterFile",
        "AdapterRoute",
        "Agent",
        "Answer",
        "Answered",
        "CallFailed",
        "CallRefused",
        "CallResult",
        "CapabilityError",
        "CeilingExceeded",
        "Ctx",
        "Ended",
        "EngineError",
        "Failed",
        "Failure",
        "FetchError",
        "FetchResponse",
        "FxError",
        "Group",
        "InputFile",
        "NodeFailure",
        "NodeSpec",
        "NotReceived",
        "Output",
        "Plan",
        "PlanRefused",
        "PortSpec",
        "Refused",
        "RunResult",
        "RunRecord",
        "RunObservationError",
        "RunControlError",
        "RunControlResult",
        "RunExportError",
        "RunEntry",
        "RunRemoval",
        "RunRemovalEntry",
        "RunRemovalError",
        "SpecError",
        "StandInCall",
        "StepRef",
        "Tool",
        "ToolInvocationError",
        "ToolReply",
        "ToolResult",
        "Uncertain",
        "Unreachable",
        "Workflow",
        "adapter",
        "cancel",
        "cancel_async",
        "inspect_control",
        "inspect_control_async",
        "list_runs",
        "list_runs_async",
        "load_run",
        "load_run_async",
        "node",
        "param",
        "plan",
        "plan_async",
        "remove_runs",
        "remove_runs_async",
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
