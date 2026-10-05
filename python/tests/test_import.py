import grida.fx


def test_the_sdk_imports() -> None:
    assert set(grida.fx.__all__) == {
        "Ctx",
        "Group",
        "NodeFailure",
        "NodeSpec",
        "PortSpec",
        "SpecError",
        "StepRef",
        "ToolReply",
        "Workflow",
        "node",
        "param",
        "spec_of",
        "tool",
    }


def test_the_host_module_imports() -> None:
    import grida.fx.host

    assert callable(grida.fx.host.main)
