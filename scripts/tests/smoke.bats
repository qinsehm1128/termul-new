#!/usr/bin/env bats

load "helpers.bash"

@test "bats shell harness runs" {
  run bash -c 'printf "%s\n" "se-manager shell harness"'

  [ "$status" -eq 0 ]
  [ "$output" = "se-manager shell harness" ]
}
