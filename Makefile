.PHONY: lint test clean run

lint:
	python -m flake8 src/ run_benchmark.py
	python -m mypy src/ run_benchmark.py

test:
	python -m pytest tests/ -v

clean:
	rm -rf .tari-bench-data .tari-bench-logs build dist *.egg-info
	find . -type d -name __pycache__ | xargs rm -rf

run:
	python run_benchmark.py --config config.yaml --mode all

run-new:
	python run_benchmark.py --mode new --scenario ALL

install:
	pip install -e .
