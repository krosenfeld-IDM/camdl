package tree_sitter_camdl_test

import (
	"testing"

	tree_sitter "github.com/smacker/go-tree-sitter"
	"github.com/tree-sitter/tree-sitter-camdl"
)

func TestCanLoadGrammar(t *testing.T) {
	language := tree_sitter.NewLanguage(tree_sitter_camdl.Language())
	if language == nil {
		t.Errorf("Error loading Camdl grammar")
	}
}
