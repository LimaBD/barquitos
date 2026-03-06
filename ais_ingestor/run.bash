#!/bin/bash

nohup python3 ./ais_ingestor/ingest.py --watch > /mnt/wdblack/ais_ingestor.log 2>&1 &
