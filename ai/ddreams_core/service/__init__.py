"""Domain mixins assembled into the local AI service."""

from . import mapview, suggestions
from .base import BaseMixin
from .indexing import IndexingMixin
from .faces import FacesMixin
from .suggestions import SuggestionsMixin
from .clusters import ClustersMixin
from .mapview import MapViewMixin
from .incoming import IncomingMixin
from .protocol import ProtocolMixin


class AIService(
    BaseMixin,
    IndexingMixin,
    FacesMixin,
    SuggestionsMixin,
    ClustersMixin,
    MapViewMixin,
    IncomingMixin,
    ProtocolMixin,
):
    pass


# These static methods refer to AIService by name in their original bodies.
mapview.AIService = AIService
suggestions.AIService = AIService
