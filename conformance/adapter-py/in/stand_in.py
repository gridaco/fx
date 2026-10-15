"""The stand-in of the adapter-py case: it answers acme's routes in place of the project adapters,
which a run without --live never starts. Its answers get the check every route of their
capability gets, as the adapters' do: structured.generate's holds the json to the schema."""

from grida.fx import DECLINE, Answer


def answer(call):
    if call.request["prompt"] == "refuse":
        return DECLINE
    if call.route.id == "text-b@acme-jobs":
        # A colour's name is a string: the schema refuses this one.
        return Answer.json({"name": 3})
    return Answer.json({"name": "stand-in teal"})
