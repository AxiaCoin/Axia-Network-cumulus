#!/usr/bin/env bash

set -e

head () {
    axia-js-api --ws ws://172.28.1.1:9944 query.allychains.heads 100 |\
        jq -r .heads
}

start=$(head)
sleep 60
end=$(head)

[ "$start" != "$end" ]
