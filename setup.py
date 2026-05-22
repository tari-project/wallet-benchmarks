from setuptools import setup, find_packages

setup(
    name="tari-wallet-benchmarks",
    version="1.0.0",
    packages=find_packages(where="src"),
    package_dir={"": "src"},
    python_requires=">=3.11",
    install_requires=[
        "pyyaml>=6.0",
        "httpx>=0.27",
        "grpcio>=1.62",
        "grpcio-tools>=1.62",
        "psutil>=5.9",
    ],
    entry_points={
        "console_scripts": [
            "tari-bench=run_benchmark:main",
        ],
    },
)
