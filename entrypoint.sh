#!/bin/sh
set -e

if [ -n "$DEMO_DATA_SEED" ]; then
    if [ ! -f /data/demodata/FactSales.parquet ]; then
        echo "DEMO_DATA_SEED=$DEMO_DATA_SEED — generating demo data..."
        cd /data && generate-demodata "$DEMO_DATA_SEED"
    else
        echo "Demo data already exists, skipping generation."
    fi
fi

exec dax-rs "$@"
