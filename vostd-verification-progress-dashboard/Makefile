PROGRESS_JSON ?= ../target/verification-progress/progress.json

.PHONY: all refresh check clean

all: index.html

$(PROGRESS_JSON):
	$(MAKE) -C .. progress

index.html: $(PROGRESS_JSON) generate_html.py template.html
	python3 generate_html.py --input $(PROGRESS_JSON) --output $@

refresh:
	$(MAKE) -C .. progress
	python3 generate_html.py --input $(PROGRESS_JSON) --output index.html

check: index.html
	python3 generate_html.py --input $(PROGRESS_JSON) --output index.html --check

clean:
	rm -f index.html
